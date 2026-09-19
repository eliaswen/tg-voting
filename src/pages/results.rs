use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::Row;

use crate::{pages::login::AppState, render::render_template_page};

#[derive(Deserialize, Default)]
pub struct ReceiptSearch {
    #[serde(default)]
    q: String,
    #[serde(default)]
    page: i64,
}

struct Winner { contest: String, name: String }
struct Certification { contest: String, actor: String, certified_at: String, reason: String }
struct Receipt { number: String, round: String, presidential: String, council: String, ombudsman: String }

#[derive(Template)]
#[template(path = "elections/results.html")]
struct ResultsPage<'a> {
    name: &'a str,
    certified: bool,
    winners: &'a [Winner],
    certifications: &'a [Certification],
    election_uuid: uuid::Uuid,
}

#[derive(Template)]
#[template(path = "elections/receipts.html")]
struct ReceiptsPage<'a> {
    election_uuid: uuid::Uuid,
    receipts: &'a [Receipt],
    query: &'a str,
    page: i64,
    has_next: bool,
}

pub async fn get_results(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let election = match sqlx::query("SELECT name, status::text AS status FROM elections WHERE uuid = $1")
        .bind(election_uuid).fetch_optional(&state.pool).await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let certified = election.get::<String, _>("status") == "certified";
    let mut winners = Vec::new();
    let mut certifications = Vec::new();
    if certified {
        let rows = match sqlx::query(
            "WITH current_results AS (
                 SELECT DISTINCT ON (contest) uuid, contest FROM election_results
                 WHERE election_id = $1 AND status = 'certified'
                 ORDER BY contest, round_number DESC
             )
             SELECT contest, name FROM (
                 SELECT current.contest::text AS contest,
                        president.election_display_name || ' / ' || vice.election_display_name AS name
                 FROM current_results current
                 JOIN election_result_tickets elected ON elected.result_id = current.uuid AND elected.elected
                 JOIN presidential_tickets tickets ON tickets.uuid = elected.ticket_id
                 JOIN candidates president ON president.uuid = tickets.president_candidate_id
                 JOIN candidates vice ON vice.uuid = tickets.vice_president_candidate_id
                 WHERE current.contest = 'president'
                 UNION ALL
                 SELECT current.contest::text, candidates.election_display_name
                 FROM current_results current
                 JOIN election_result_candidates elected ON elected.result_id = current.uuid AND elected.elected
                 JOIN candidates ON candidates.uuid = elected.candidate_id
                 WHERE current.contest <> 'president'
             ) winners ORDER BY contest, name",
        ).bind(election_uuid).fetch_all(&state.pool).await {
            Ok(rows) => rows,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        winners = rows.into_iter().map(|row| Winner { contest: row.get("contest"), name: row.get("name") }).collect();
        let rows = match sqlx::query(
            "WITH current_results AS (
                 SELECT DISTINCT ON (contest) * FROM election_results
                 WHERE election_id = $1 AND status = 'certified'
                 ORDER BY contest, round_number DESC
             )
             SELECT results.contest::text AS contest,
                    COALESCE(NULLIF(identities.preferred_username, ''),
                             NULLIF(identities.display_name, ''),
                             'Election manager') AS actor,
                    to_char(results.certified_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI UTC') AS certified_at,
                    results.certification_reason AS reason
             FROM current_results results JOIN citizens ON citizens.uuid = results.certified_by
             LEFT JOIN authentik_identities identities ON identities.citizen_id = citizens.uuid
             ORDER BY results.contest",
        ).bind(election_uuid).fetch_all(&state.pool).await {
            Ok(rows) => rows,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        certifications = rows.into_iter().map(|row| Certification {
            contest: row.get("contest"), actor: row.get("actor"),
            certified_at: row.get("certified_at"), reason: row.get("reason"),
        }).collect();
    }
    render_template_page(
        &ResultsPage { name: election.get("name"), certified, winners: &winners, certifications: &certifications, election_uuid },
        "Election results", jar, &state.pool,
    ).await.into_response()
}

pub async fn get_receipts(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Query(search): Query<ReceiptSearch>,
) -> Response {
    let status = match sqlx::query_scalar::<_, String>("SELECT status::text FROM elections WHERE uuid = $1")
        .bind(election_uuid).fetch_optional(&state.pool).await
    {
        Ok(Some(status)) => status,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if status != "certified" { return StatusCode::NOT_FOUND.into_response(); }
    let page = search.page.max(0);
    let needle = search.q.trim();
    let rows = match sqlx::query(
        "WITH current_results AS (
             SELECT DISTINCT ON (contest) contest, revote_id FROM election_results
             WHERE election_id = $1 AND status = 'certified'
             ORDER BY contest, round_number DESC
         )
         SELECT ballots.receipt_number,
                COALESCE(ballots.revote_id::text, 'initial round') AS round,
                CASE WHEN EXISTS (SELECT 1 FROM current_results WHERE contest = 'president' AND revote_id IS NOT DISTINCT FROM ballots.revote_id)
                     THEN COALESCE((SELECT string_agg(president.election_display_name || ' / ' || vice.election_display_name, ', ' ORDER BY votes.ranking)
                         FROM presidential_votes votes JOIN presidential_tickets tickets ON tickets.uuid = votes.ticket_id
                         JOIN candidates president ON president.uuid = tickets.president_candidate_id
                         JOIN candidates vice ON vice.uuid = tickets.vice_president_candidate_id
                         WHERE votes.ballot_uuid = ballots.uuid), 'Abstained')
                     ELSE 'Not counted in this round' END AS presidential,
                CASE WHEN EXISTS (SELECT 1 FROM current_results WHERE contest = 'council' AND revote_id IS NOT DISTINCT FROM ballots.revote_id)
                     THEN COALESCE((SELECT string_agg(candidates.election_display_name, ', ' ORDER BY candidates.election_display_name)
                         FROM candidate_votes votes JOIN candidates ON candidates.uuid = votes.candidate_id
                         WHERE votes.ballot_uuid = ballots.uuid AND votes.position = 'council'), 'Abstained')
                     ELSE 'Not counted in this round' END AS council,
                CASE WHEN EXISTS (SELECT 1 FROM current_results WHERE contest = 'ombudsman' AND revote_id IS NOT DISTINCT FROM ballots.revote_id)
                     THEN COALESCE((SELECT string_agg(candidates.election_display_name, ', ')
                         FROM candidate_votes votes JOIN candidates ON candidates.uuid = votes.candidate_id
                         WHERE votes.ballot_uuid = ballots.uuid AND votes.position = 'ombudsman'), 'Abstained')
                     ELSE 'Not counted in this round' END AS ombudsman
         FROM ballots
         WHERE ballots.election_id = $1 AND ballots.receipt_number ILIKE $2
           AND EXISTS (SELECT 1 FROM current_results WHERE revote_id IS NOT DISTINCT FROM ballots.revote_id)
         ORDER BY ballots.receipt_number LIMIT 101 OFFSET $3",
    ).bind(election_uuid).bind(format!("%{needle}%")).bind(page * 100).fetch_all(&state.pool).await {
        Ok(rows) => rows,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let has_next = rows.len() > 100;
    let receipts = rows.into_iter().take(100).map(|row| Receipt {
        number: row.get("receipt_number"), round: row.get("round"),
        presidential: row.get("presidential"), council: row.get("council"), ombudsman: row.get("ombudsman"),
    }).collect::<Vec<_>>();
    render_template_page(
        &ReceiptsPage { election_uuid, receipts: &receipts, query: needle, page, has_next },
        "Counted ballot receipts", jar, &state.pool,
    ).await.into_response()
}
