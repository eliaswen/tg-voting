use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::{Form, cookie::CookieJar};
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;

use crate::{
    pages::{auth::require_election_manager, login::AppState},
    render::render_template_page,
};

#[derive(Debug, Deserialize)]
pub struct ReviewForm {
    contest: String,
    #[serde(default)]
    candidate_id: Vec<uuid::Uuid>,
    #[serde(default)]
    action: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RevoteForm {
    mode: String,
    #[serde(default)]
    contest: Option<String>,
    reason: String,
    #[serde(default)]
    code_issuance_starts_at: String,
    #[serde(default)]
    registration_starts_at: String,
    #[serde(default)]
    registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    #[serde(default)]
    confirm_delete: String,
}

struct ReviewResult {
    contest: String,
    status: String,
    reviewed: bool,
    details: String,
    candidates: Vec<ReviewCandidate>,
}

struct ReviewCandidate {
    id: uuid::Uuid,
    name: String,
    selected: bool,
    tied: bool,
    votes: i32,
}

struct ReviewHistory {
    contest: String,
    round: i32,
    status: String,
    revote: String,
    details: String,
}

#[derive(Template)]
#[template(path = "manage-election-status/results.html")]
struct ReviewPage<'a> {
    election_uuid: uuid::Uuid,
    election_name: &'a str,
    results: &'a [ReviewResult],
    history: &'a [ReviewHistory],
}

pub async fn get_result_review(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    let election = match sqlx::query("SELECT name FROM elections WHERE uuid = $1")
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let rows = match sqlx::query(
        "SELECT DISTINCT ON (contest) uuid, contest::text AS contest, status,
                reviewed_at IS NOT NULL AS reviewed, count_details::text AS details
         FROM election_results WHERE election_id = $1 AND status <> 'voided'
         ORDER BY contest, round_number DESC",
    )
    .bind(election_uuid)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut results = Vec::new();
    for row in rows {
        let result_id: uuid::Uuid = row.get("uuid");
        let contest: String = row.get("contest");
        let option_rows = if contest == "president" {
            sqlx::query(
                "SELECT options.ticket_id AS id,
                        president.election_display_name || ' / ' || vice.election_display_name AS name,
                        options.elected, options.tied, options.votes
                 FROM election_result_tickets options
                 JOIN presidential_tickets tickets ON tickets.uuid = options.ticket_id
                 JOIN candidates president ON president.uuid = tickets.president_candidate_id
                 JOIN candidates vice ON vice.uuid = tickets.vice_president_candidate_id
                 WHERE options.result_id = $1 ORDER BY name",
            )
            .bind(result_id)
            .fetch_all(&state.pool)
            .await
        } else {
            sqlx::query(
                "SELECT options.candidate_id AS id, candidates.election_display_name AS name,
                        options.elected, options.tied, options.votes
                 FROM election_result_candidates options
                 JOIN candidates ON candidates.uuid = options.candidate_id
                 WHERE options.result_id = $1 ORDER BY name",
            )
            .bind(result_id)
            .fetch_all(&state.pool)
            .await
        };
        let option_rows = match option_rows {
            Ok(rows) => rows,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let candidates = option_rows.into_iter().map(|candidate| ReviewCandidate {
            id: candidate.get("id"),
            name: candidate.get("name"),
            selected: candidate.get("elected"),
            tied: candidate.get("tied"),
            votes: candidate.get("votes"),
        }).collect();
        results.push(ReviewResult {
            contest,
            status: row.get("status"),
            reviewed: row.get("reviewed"),
            details: row.get("details"),
            candidates,
        });
    }
    let history = match sqlx::query(
        "SELECT contest::text AS contest, round_number AS round, status,
                COALESCE(revote_id::text, 'initial round') AS revote,
                count_details::text AS details
         FROM election_results WHERE election_id = $1 ORDER BY contest, round_number",
    )
    .bind(election_uuid)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows.into_iter().map(|row| ReviewHistory {
            contest: row.get("contest"),
            round: row.get("round"),
            status: row.get("status"),
            revote: row.get("revote"),
            details: row.get("details"),
        }).collect::<Vec<_>>(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    render_template_page(
        &ReviewPage { election_uuid, election_name: election.get("name"), results: &results, history: &history },
        "Review election results",
        jar,
        &state.pool,
    ).await.into_response()
}

pub async fn post_result_review(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<ReviewForm>,
) -> Response {
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if !matches!(form.contest.as_str(), "president" | "council" | "ombudsman")
        || form.reason.trim().is_empty()
        || !matches!(form.action.as_str(), "accept" | "override" | "eliminate")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let election = match sqlx::query(
        "SELECT status::text AS status, active_revote_id FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(Some(row)) if matches!(row.get::<String, _>("status").as_str(), "counting" | "closed") => row,
        Ok(Some(_)) => return StatusCode::CONFLICT.into_response(),
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result = match sqlx::query(
        "SELECT uuid, status FROM election_results
         WHERE election_id = $1 AND contest = $2::candidate_position AND status <> 'voided'
         ORDER BY round_number DESC LIMIT 1 FOR UPDATE",
    )
    .bind(election_uuid)
    .bind(&form.contest)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(Some(row)) if row.get::<String, _>("status") != "certified" => row,
        Ok(Some(_)) => return StatusCode::CONFLICT.into_response(),
        Ok(None) => return StatusCode::CONFLICT.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result_id: uuid::Uuid = result.get("uuid");
    let result_status: String = result.get("status");

    if form.action == "eliminate" {
        if form.contest != "president" || result_status != "review" || form.candidate_id.len() != 1 {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let tied = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM election_result_tickets
             WHERE result_id = $1 AND ticket_id = $2 AND tied)",
        ).bind(result_id).bind(form.candidate_id[0]).fetch_one(&mut *transaction).await.unwrap_or(false);
        if !tied { return StatusCode::BAD_REQUEST.into_response(); }
        if sqlx::query(
            "INSERT INTO election_result_actions (result_id, actor_id, action, reason, ticket_id)
             VALUES ($1, $2, 'eliminate', $3, $4)",
        ).bind(result_id).bind(actor.uuid).bind(form.reason.trim()).bind(form.candidate_id[0])
            .execute(&mut *transaction).await.is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        if crate::election_results::recount_president_in_transaction(
            &mut transaction, election_uuid, election.get("active_revote_id")
        ).await.is_err() || transaction.commit().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        return Redirect::to(&format!("/manage/elections/{election_uuid}/results")).into_response();
    }

    if form.action == "accept" && result_status != "provisional" {
        return StatusCode::CONFLICT.into_response();
    }

    let selected = if form.action == "accept" {
        if form.contest == "president" {
            sqlx::query_scalar("SELECT ticket_id FROM election_result_tickets WHERE result_id = $1 AND elected")
                .bind(result_id).fetch_all(&mut *transaction).await
        } else {
            sqlx::query_scalar("SELECT candidate_id FROM election_result_candidates WHERE result_id = $1 AND elected")
                .bind(result_id).fetch_all(&mut *transaction).await
        }
    } else {
        Ok(form.candidate_id.clone())
    };
    let selected: Vec<uuid::Uuid> = match selected {
        Ok(selected) => selected,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let seats = sqlx::query_scalar::<_, i32>("SELECT council_seats FROM elections WHERE uuid = $1")
        .bind(election_uuid).fetch_one(&mut *transaction).await.unwrap_or(1).max(1) as usize;
    if selected.is_empty()
        || (form.contest == "council" && selected.len() > seats)
        || (form.contest != "council" && selected.len() != 1)
        || selected.iter().collect::<HashSet<_>>().len() != selected.len()
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let valid = if form.contest == "president" {
        sqlx::query_scalar::<_, bool>(
            "SELECT COUNT(*) = $3 FROM election_result_tickets
             WHERE result_id = $1 AND ticket_id = ANY($2)",
        ).bind(result_id).bind(&selected).bind(selected.len() as i64).fetch_one(&mut *transaction).await
    } else {
        sqlx::query_scalar::<_, bool>(
            "SELECT COUNT(*) = $3 FROM election_result_candidates
             WHERE result_id = $1 AND candidate_id = ANY($2)",
        ).bind(result_id).bind(&selected).bind(selected.len() as i64).fetch_one(&mut *transaction).await
    }.unwrap_or(false);
    if !valid { return StatusCode::BAD_REQUEST.into_response(); }
    let update = if form.contest == "president" {
        sqlx::query("UPDATE election_result_tickets SET elected = ticket_id = ANY($2), tied = FALSE WHERE result_id = $1")
            .bind(result_id).bind(&selected).execute(&mut *transaction).await
    } else {
        sqlx::query("UPDATE election_result_candidates SET elected = candidate_id = ANY($2), tied = FALSE WHERE result_id = $1")
            .bind(result_id).bind(&selected).execute(&mut *transaction).await
    };
    if update.is_err()
        || sqlx::query(
            "UPDATE election_results SET status = 'provisional', reviewed_at = clock_timestamp(),
                    reviewed_by = $2, review_reason = $3 WHERE uuid = $1",
        ).bind(result_id).bind(actor.uuid).bind(form.reason.trim()).execute(&mut *transaction).await.is_err()
        || sqlx::query(
            "INSERT INTO election_result_actions (result_id, actor_id, action, reason)
             VALUES ($1, $2, $3, $4)",
        ).bind(result_id).bind(actor.uuid)
            .bind(if form.action == "accept" { "accepted" } else { "minister override" })
            .bind(form.reason.trim()).execute(&mut *transaction).await.is_err()
        || transaction.commit().await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::to(&format!("/manage/elections/{election_uuid}/results")).into_response()
}

pub async fn post_revote(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<RevoteForm>,
) -> Response {
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let timezone = crate::render::timezone(&jar);
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let election = match sqlx::query("SELECT status::text AS status FROM elections WHERE uuid = $1 FOR UPDATE")
        .bind(election_uuid).fetch_optional(&mut *transaction).await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if form.mode == "full_restart" {
        return full_restart(transaction, election_uuid, form, &timezone).await;
    }
    let Some(contest) = valid_ordinary_revote(&form) else { return StatusCode::BAD_REQUEST.into_response(); };
    if !matches!(election.get::<String, _>("status").as_str(), "counting" | "closed") {
        return StatusCode::CONFLICT.into_response();
    }
    if contest.is_none() {
        let missing = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM election_positions positions
             WHERE positions.election_id = $1
               AND positions.position IN ('president', 'council', 'ombudsman')
               AND NOT EXISTS (SELECT 1 FROM election_results results
                               WHERE results.election_id = positions.election_id
                                 AND results.contest = positions.position AND results.status <> 'voided')",
        ).bind(election_uuid).fetch_one(&mut *transaction).await;
        match missing {
            Ok(0) => {}
            Ok(_) => return StatusCode::CONFLICT.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
    let source_result = if let Some(contest) = contest {
        match current_result(&mut transaction, election_uuid, contest).await {
            Ok(Some(row)) => Some(row),
            Ok(None) => return StatusCode::CONFLICT.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    } else { None };
    if source_result.as_ref().is_some_and(|row| row.get::<String, _>("status") == "certified") {
        return StatusCode::CONFLICT.into_response();
    }
    if form.mode == "runoff" && !source_result.as_ref().is_some_and(|row| row.get::<String, _>("status") == "runoff") {
        return StatusCode::CONFLICT.into_response();
    }
    let source_id = source_result.as_ref().map(|row| row.get::<uuid::Uuid, _>("uuid"));
    let runoff_source_id = (form.mode == "runoff").then_some(source_id).flatten();
    let unresolved_seats = if form.mode == "runoff" && contest == Some("council") {
        source_result.as_ref().and_then(|row| row.get::<serde_json::Value, _>("count_details")["unresolved_seats"].as_u64()).map(|value| value as i32)
    } else { None };
    let revote = match sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO election_revotes
             (election_id, contest, mode, reason, requested_by, source_result_id,
              unresolved_seats, voting_starts_at, voting_ends_at)
         VALUES ($1, $2::candidate_position, $3::election_revote_mode, $4, $5, $6, $7,
                 $8::timestamp AT TIME ZONE $10, $9::timestamp AT TIME ZONE $10)
         RETURNING uuid",
    ).bind(election_uuid).bind(contest).bind(&form.mode).bind(form.reason.trim())
        .bind(actor.uuid).bind(runoff_source_id).bind(unresolved_seats).bind(&form.voting_starts_at)
        .bind(&form.voting_ends_at).bind(&timezone).fetch_one(&mut *transaction).await
    {
        Ok(value) => value,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if snapshot_revote_options(&mut transaction, election_uuid, revote, &form.mode, contest, runoff_source_id).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let voided = if let Some(contest) = contest {
        sqlx::query(
            "UPDATE election_results SET status = 'voided'
             WHERE uuid = (SELECT uuid FROM election_results WHERE election_id = $1
                           AND contest = $2::candidate_position AND status <> 'voided'
                           ORDER BY round_number DESC LIMIT 1)",
        ).bind(election_uuid).bind(contest).execute(&mut *transaction).await
    } else {
        sqlx::query(
            "UPDATE election_results SET status = 'voided' WHERE uuid IN
             (SELECT DISTINCT ON (contest) uuid FROM election_results
              WHERE election_id = $1 AND status <> 'voided' ORDER BY contest, round_number DESC)",
        ).bind(election_uuid).execute(&mut *transaction).await
    };
    if voided.is_err()
        || sqlx::query("UPDATE election_eligibility SET credential_issued = FALSE WHERE election_id = $1")
            .bind(election_uuid).execute(&mut *transaction).await.is_err()
        || sqlx::query(
            "UPDATE elections SET active_revote_id = $2, status = 'upcoming',
                    voter_code_registration_starts_at = $3::timestamp AT TIME ZONE $6,
                    voter_code_registration_ends_at = $5::timestamp AT TIME ZONE $6,
                    registration_starts_at = $4::timestamp AT TIME ZONE $6,
                    registration_ends_at = $4::timestamp AT TIME ZONE $6,
                    voting_starts_at = $4::timestamp AT TIME ZONE $6,
                    voting_ends_at = $5::timestamp AT TIME ZONE $6,
                    paused_at = NULL, paused_stage = NULL, expected_resume_at = NULL
             WHERE uuid = $1",
        ).bind(election_uuid).bind(revote).bind(&form.code_issuance_starts_at)
            .bind(&form.voting_starts_at).bind(&form.voting_ends_at).bind(&timezone)
            .execute(&mut *transaction).await.is_err()
        || transaction.commit().await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::to(&format!("/manage/elections/{election_uuid}/results")).into_response()
}

async fn current_result(
    transaction: &mut Transaction<'_, Postgres>,
    election_uuid: uuid::Uuid,
    contest: &str,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(
        "SELECT uuid, status, count_details FROM election_results
         WHERE election_id = $1 AND contest = $2::candidate_position AND status <> 'voided'
         ORDER BY round_number DESC LIMIT 1 FOR UPDATE",
    ).bind(election_uuid).bind(contest).fetch_optional(&mut **transaction).await
}

async fn snapshot_revote_options(
    transaction: &mut Transaction<'_, Postgres>,
    election_uuid: uuid::Uuid,
    revote_id: uuid::Uuid,
    mode: &str,
    contest: Option<&str>,
    source_result: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    if contest.is_none() || contest == Some("president") {
        if mode == "runoff" {
            sqlx::query(
                "INSERT INTO election_revote_tickets (revote_id, ticket_id)
                 SELECT $1, ticket_id FROM election_result_tickets WHERE result_id = $2 AND tied",
            ).bind(revote_id).bind(source_result).execute(&mut **transaction).await?;
        } else {
            sqlx::query(
                "INSERT INTO election_revote_tickets (revote_id, ticket_id)
                 SELECT $1, uuid FROM presidential_tickets WHERE election_id = $2 AND status = 'active'",
            ).bind(revote_id).bind(election_uuid).execute(&mut **transaction).await?;
        }
    }
    for position in ["council", "ombudsman"] {
        if contest.is_some() && contest != Some(position) { continue; }
        if mode == "runoff" {
            sqlx::query(
                "INSERT INTO election_revote_candidates (revote_id, contest, candidate_id)
                 SELECT $1, $2::candidate_position, candidate_id FROM election_result_candidates
                 WHERE result_id = $3 AND tied",
            ).bind(revote_id).bind(position).bind(source_result).execute(&mut **transaction).await?;
        } else {
            sqlx::query(
                "INSERT INTO election_revote_candidates (revote_id, contest, candidate_id)
                 SELECT $1, $2::candidate_position, uuid FROM candidates
                 WHERE election_id = $3 AND position = $2::candidate_position AND status = 'active'",
            ).bind(revote_id).bind(position).bind(election_uuid).execute(&mut **transaction).await?;
        }
    }
    Ok(())
}

async fn full_restart(
    mut transaction: Transaction<'_, Postgres>,
    election_uuid: uuid::Uuid,
    form: RevoteForm,
    timezone: &str,
) -> Response {
    if !valid_full_restart(&form) { return StatusCode::BAD_REQUEST.into_response(); }
    if sqlx::query("UPDATE elections SET active_revote_id = NULL WHERE uuid = $1")
        .bind(election_uuid).execute(&mut *transaction).await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    for statement in [
        "DELETE FROM ballots WHERE election_id = $1",
        "DELETE FROM voting_codes WHERE election_id = $1",
        "DELETE FROM election_results WHERE election_id = $1",
        "DELETE FROM election_revotes WHERE election_id = $1",
        "DELETE FROM election_eligibility WHERE election_id = $1",
        "DELETE FROM presidential_tickets WHERE election_id = $1",
        "DELETE FROM candidates WHERE election_id = $1",
    ] {
        if sqlx::query(statement).bind(election_uuid).execute(&mut *transaction).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if sqlx::query(
        "UPDATE elections SET status = 'draft', published_at = NULL,
                registration_starts_at = $2::timestamp AT TIME ZONE $6,
                registration_ends_at = $3::timestamp AT TIME ZONE $6,
                voting_starts_at = $4::timestamp AT TIME ZONE $6,
                voting_ends_at = $5::timestamp AT TIME ZONE $6,
                voter_code_registration_starts_at = NULL, voter_code_registration_ends_at = NULL,
                paused_at = NULL, paused_stage = NULL, expected_resume_at = NULL,
                eligibility_snapshotted_at = NULL WHERE uuid = $1",
    ).bind(election_uuid).bind(&form.registration_starts_at).bind(&form.registration_ends_at)
        .bind(&form.voting_starts_at).bind(&form.voting_ends_at).bind(timezone)
        .execute(&mut *transaction).await.is_err() || transaction.commit().await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

fn valid_ordinary_revote(form: &RevoteForm) -> Option<Option<&str>> {
    let parse = |value: &str| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").ok();
    let (Some(code_start), Some(voting_start), Some(voting_end)) = (
        parse(&form.code_issuance_starts_at), parse(&form.voting_starts_at), parse(&form.voting_ends_at)
    ) else { return None; };
    if form.reason.trim().is_empty()
        || !matches!(form.mode.as_str(), "all_contests" | "selected_contest" | "runoff")
        || !(code_start <= voting_start && voting_start < voting_end)
    { return None; }
    let contest = form.contest.as_deref().filter(|contest| matches!(*contest, "president" | "council" | "ombudsman"));
    ((matches!(form.mode.as_str(), "selected_contest" | "runoff")) == contest.is_some()).then_some(contest)
}

fn valid_full_restart(form: &RevoteForm) -> bool {
    let parse = |value: &str| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").ok();
    let (Some(registration_start), Some(registration_end), Some(voting_start), Some(voting_end)) = (
        parse(&form.registration_starts_at), parse(&form.registration_ends_at),
        parse(&form.voting_starts_at), parse(&form.voting_ends_at)
    ) else { return false; };
    form.mode == "full_restart" && form.reason.trim().len() > 0 && form.confirm_delete == "DELETE"
        && registration_start < registration_end && registration_end <= voting_start && voting_start < voting_end
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, extract::FromRequest, http::Request};

    fn valid_form() -> RevoteForm {
        RevoteForm {
            mode: "selected_contest".to_string(), contest: Some("council".to_string()),
            reason: "Tie".to_string(), code_issuance_starts_at: "2026-01-01T10:00".to_string(),
            registration_starts_at: String::new(), registration_ends_at: String::new(),
            voting_starts_at: "2026-01-03T10:00".to_string(), voting_ends_at: "2026-01-04T10:00".to_string(),
            confirm_delete: String::new(),
        }
    }

    #[test]
    fn selected_revote_requires_exactly_one_supported_contest() {
        let valid = valid_form();
        assert_eq!(valid_ordinary_revote(&valid), Some(Some("council")));
        assert_eq!(valid_ordinary_revote(&RevoteForm { mode: "runoff".to_string(), ..valid.clone() }), Some(Some("council")));
        assert_eq!(valid_ordinary_revote(&RevoteForm { contest: None, ..valid.clone() }), None);
        assert_eq!(valid_ordinary_revote(&RevoteForm { mode: "all_contests".to_string(), ..valid.clone() }), None);
        assert_eq!(valid_ordinary_revote(&RevoteForm { voting_ends_at: "2026-01-03T09:00".to_string(), ..valid }), None);
    }

    #[test]
    fn full_restart_requires_explicit_confirmation_and_complete_schedule() {
        let form = RevoteForm {
            mode: "full_restart".to_string(), contest: None, reason: "Start over".to_string(),
            code_issuance_starts_at: String::new(), registration_starts_at: "2026-01-01T10:00".to_string(),
            registration_ends_at: "2026-01-02T10:00".to_string(), voting_starts_at: "2026-01-03T10:00".to_string(),
            voting_ends_at: "2026-01-04T10:00".to_string(), confirm_delete: "DELETE".to_string(),
        };
        assert!(valid_full_restart(&form));
        assert!(!valid_full_restart(&RevoteForm { confirm_delete: String::new(), ..form }));
    }

    #[tokio::test]
    async fn repeated_candidate_fields_use_html_form_decoding() {
        let request = Request::builder()
            .method("POST")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(
                "contest=council&action=override&reason=Review&candidate_id=00000000-0000-0000-0000-000000000001&candidate_id=00000000-0000-0000-0000-000000000002",
            ))
            .unwrap();
        let Form(form) = Form::<ReviewForm>::from_request(request, &()).await.unwrap();
        assert_eq!(form.candidate_id.len(), 2);
        assert_eq!(form.candidate_id[0], uuid::Uuid::from_u128(1));
        assert_eq!(form.candidate_id[1], uuid::Uuid::from_u128(2));
    }
}
