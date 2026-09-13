use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{Form, cookie::CookieJar};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;
use tracing::{error, info};

use crate::error_handling::{ErrorPage, error_response};
use crate::pages::auth::require_citizen;
use crate::pages::election_lifecycle::timeline;
use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Deserialize)]
pub struct CitizenIdForm {
    citizen_id: String,
    #[serde(default)]
    debug_census: bool,
}

#[derive(Deserialize)]
pub struct VotingCodeForm {
    voting_code: String,
}

#[derive(Deserialize)]
pub struct BallotForm {
    voting_code: String,
    #[serde(default)]
    presidential_ticket: Vec<uuid::Uuid>,
    #[serde(default)]
    presidential_ranking: Vec<i32>,
    #[serde(default)]
    council_candidate: Vec<uuid::Uuid>,
    #[serde(default)]
    abstain_position: Vec<String>,
    #[serde(default)]
    candidate_id: Vec<uuid::Uuid>,
}

struct TicketChoice {
    uuid: uuid::Uuid,
    president_uuid: uuid::Uuid,
    president_name: String,
    vice_president_uuid: uuid::Uuid,
    vice_president_name: String,
    party: String,
}
struct CandidateChoice {
    uuid: uuid::Uuid,
    user_uuid: uuid::Uuid,
    name: String,
    party: String,
}
struct PositionChoice {
    position: String,
    label: String,
    candidates: Vec<CandidateChoice>,
}
struct BallotChoices {
    tickets: Vec<TicketChoice>,
    council_candidates: Vec<CandidateChoice>,
    positions: Vec<PositionChoice>,
    council_seats: i32,
}

fn round_id(election: &sqlx::postgres::PgRow) -> Option<uuid::Uuid> {
    election.get("active_revote_id")
}

#[derive(Template)]
#[template(path = "elections/voter-code.html")]
struct VoterCodePage<'a> {
    election_uuid: uuid::Uuid,
    election_name: &'a str,
    issued: bool,
    code: &'a str,
    debug_mode: bool,
}

#[derive(Template)]
#[template(path = "elections/vote.html")]
struct VotePage<'a> {
    election_uuid: uuid::Uuid,
    election_name: &'a str,
    confirming: bool,
    voting_code: &'a str,
    receipt: &'a str,
    debug_bypass: bool,
    tickets: &'a [TicketChoice],
    council_candidates: &'a [CandidateChoice],
    positions: &'a [PositionChoice],
    council_seats: i32,
}

pub async fn get_voter_code(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if !code_requests_open(&election) {
        return bad_request("Voter codes are not available at this stage.");
    }
    if let Err(response) = ensure_snapshot(&state, election_uuid).await {
        return response;
    }
    let issued = sqlx::query_scalar::<_, bool>("SELECT credential_issued FROM election_eligibility WHERE election_id = $1 AND citizen_id = $2")
        .bind(election_uuid).bind(citizen.uuid).fetch_optional(&state.pool).await.ok().flatten().unwrap_or(false);
    render_template_page(
        &VoterCodePage {
            election_uuid,
            election_name: election.get("name"),
            issued,
            code: "",
            debug_mode: state.app_mode == 0,
        },
        "Obtain voter code",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn post_voter_code(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<CitizenIdForm>,
) -> Response {
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if !code_requests_open(&election) {
        return bad_request("Voter codes are not available at this stage.");
    }
    if form.citizen_id.len() != 6
        || !form
            .citizen_id
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return bad_request("The citizen ID must contain six numbers.");
    }
    let matches_account = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM citizens WHERE uuid = $1 AND citizen_id = $2)",
    )
    .bind(citizen.uuid)
    .bind(&form.citizen_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);
    if !matches_account {
        return bad_request("Invalid citizen ID.");
    }
    let census_bypass = state.app_mode == 0 && form.debug_census;
    if !census_bypass {
        if let Err(response) = ensure_snapshot(&state, election_uuid).await {
            return response;
        }
    }
    if census_bypass {
        let _ = sqlx::query("INSERT INTO election_eligibility (election_id, citizen_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(election_uuid).bind(citizen.uuid).execute(&state.pool).await;
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return database_error(error),
    };
    let locked_round = match sqlx::query_scalar::<_, Option<uuid::Uuid>>(
        "SELECT active_revote_id FROM elections
         WHERE uuid = $1 AND status IN ('upcoming', 'voting')
           AND clock_timestamp() >= COALESCE(voter_code_registration_starts_at, registration_starts_at)
           AND clock_timestamp() < COALESCE(voter_code_registration_ends_at, voting_ends_at)
         FOR SHARE",
    ).bind(election_uuid).fetch_optional(&mut *transaction).await {
        Ok(Some(round)) if round == round_id(&election) => round,
        _ => return bad_request("Voter codes are not available at this stage."),
    };
    let eligible = sqlx::query_scalar::<_, bool>("SELECT credential_issued FROM election_eligibility WHERE election_id = $1 AND citizen_id = $2 FOR UPDATE")
        .bind(election_uuid).bind(citizen.uuid).fetch_optional(&mut *transaction).await;
    match eligible {
        Ok(Some(false)) => {}
        Ok(Some(true)) => {
            return bad_request("A voter code has already been issued for this election.");
        }
        Ok(None) => {
            return bad_request("You are not eligible to obtain a voter code for this election.");
        }
        Err(error) => return database_error(error),
    }
    let code = random_code();
    let code_hash = expensive_hash(&code, &uuid::Uuid::new_v4().simple().to_string());
    let code_lookup_hash = Sha256::digest(code.as_bytes()).to_vec();
    if let Err(error) = sqlx::query(
        "INSERT INTO voting_codes (election_id, revote_id, code_hash, code_lookup_hash) VALUES ($1, $2, $3, $4)",
    )
    .bind(election_uuid)
    .bind(locked_round)
    .bind(code_hash)
    .bind(code_lookup_hash)
    .execute(&mut *transaction)
    .await
    {
        return database_error(error);
    }
    if let Err(error) = sqlx::query("UPDATE election_eligibility SET credential_issued = TRUE WHERE election_id = $1 AND citizen_id = $2").bind(election_uuid).bind(citizen.uuid).execute(&mut *transaction).await { return database_error(error); }
    if let Err(error) = transaction.commit().await {
        return database_error(error);
    }
    info!(%election_uuid, citizen_id = citizen.id, "Issued anonymous voter code");
    render_template_page(
        &VoterCodePage {
            election_uuid,
            election_name: election.get("name"),
            issued: true,
            code: &code,
            debug_mode: state.app_mode == 0,
        },
        "Your voter code",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn get_vote(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let choices = match load_choices(&state, election_uuid).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    render_template_page(
        &VotePage {
            election_uuid,
            election_name: election.get("name"),
            confirming: false,
            voting_code: "",
            receipt: "",
            debug_bypass: state.app_mode == 0,
            tickets: &choices.tickets,
            council_candidates: &choices.council_candidates,
            positions: &choices.positions,
            council_seats: choices.council_seats,
        },
        "Vote",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn post_vote(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<VotingCodeForm>,
) -> Response {
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let debug_bypass = state.app_mode == 0;
    let bypassing_code = debug_bypass && normalise_code(&form.voting_code).is_empty();
    if effective_stage(&election) != "voting"
        || (!bypassing_code
            && find_code(&state, election_uuid, round_id(&election), &form.voting_code)
                .await
                .is_none())
    {
        return invalid_code();
    }
    let choices = match load_choices(&state, election_uuid).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    render_template_page(
        &VotePage {
            election_uuid,
            election_name: election.get("name"),
            confirming: true,
            voting_code: &normalise_code(&form.voting_code),
            receipt: "",
            debug_bypass,
            tickets: &choices.tickets,
            council_candidates: &choices.council_candidates,
            positions: &choices.positions,
            council_seats: choices.council_seats,
        },
        "Complete vote",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn post_complete_vote(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<BallotForm>,
) -> Response {
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if effective_stage(&election) != "voting" {
        return invalid_code();
    }
    let debug_bypass = state.app_mode == 0;
    if let Err(message) = validate_choices(&state, election_uuid, &form).await {
        return bad_request(message);
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return database_error(error),
    };
    let locked_round = match sqlx::query_scalar::<_, Option<uuid::Uuid>>(
        "SELECT active_revote_id FROM elections
         WHERE uuid = $1 AND status IN ('upcoming', 'voting')
           AND clock_timestamp() >= voting_starts_at
           AND clock_timestamp() < voting_ends_at
         FOR SHARE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(Some(round)) => round,
        _ => return invalid_code(),
    };
    if locked_round != round_id(&election) {
        return invalid_code();
    }
    let normalised = normalise_code(&form.voting_code);
    if debug_bypass && normalised.is_empty() {
        let receipt = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
        let authorization_hash = Sha256::digest(uuid::Uuid::new_v4().as_bytes()).to_vec();
        let ballot_uuid = uuid::Uuid::new_v4();
        if let Err(error) = sqlx::query("INSERT INTO ballots (uuid, election_id, revote_id, authorization_hash, receipt_number) VALUES ($1, $2, $3, $4, $5)").bind(ballot_uuid).bind(election_uuid).bind(locked_round).bind(authorization_hash).bind(&receipt).execute(&mut *transaction).await { return database_error(error); }
        if let Err(error) = store_choices(&mut transaction, ballot_uuid, &form).await {
            return database_error(error);
        }
        if let Err(error) = transaction.commit().await {
            return database_error(error);
        }
        return render_template_page(
            &VotePage {
                election_uuid,
                election_name: election.get("name"),
                confirming: false,
                voting_code: "",
                receipt: &receipt,
                debug_bypass,
                tickets: &[],
                council_candidates: &[],
                positions: &[],
                council_seats: 0,
            },
            "Vote complete",
            jar,
            &state.pool,
        )
        .await
        .into_response();
    }
    let lookup_hash = Sha256::digest(normalised.as_bytes()).to_vec();
    let code = match sqlx::query("SELECT uuid, code_hash FROM voting_codes WHERE election_id = $1 AND revote_id IS NOT DISTINCT FROM $2 AND code_lookup_hash = $3 AND used = FALSE FOR UPDATE").bind(election_uuid).bind(locked_round).bind(lookup_hash).fetch_optional(&mut *transaction).await { Ok(Some(code)) if verify_hash(&normalised, code.get("code_hash")) => code, _ => {
        return invalid_code();
    }};
    let code_uuid: uuid::Uuid = code.get("uuid");
    let receipt = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
    let authorization_hash = Sha256::digest(normalised.as_bytes()).to_vec();
    if let Err(error) = sqlx::query("UPDATE voting_codes SET used = TRUE WHERE uuid = $1")
        .bind(code_uuid)
        .execute(&mut *transaction)
        .await
    {
        return database_error(error);
    }
    let ballot_uuid = uuid::Uuid::new_v4();
    if let Err(error) = sqlx::query("INSERT INTO ballots (uuid, election_id, revote_id, authorization_hash, voting_code_uuid, receipt_number) VALUES ($1, $2, $3, $4, $5, $6)").bind(ballot_uuid).bind(election_uuid).bind(locked_round).bind(authorization_hash).bind(code_uuid).bind(&receipt).execute(&mut *transaction).await { return database_error(error); }
    if let Err(error) = store_choices(&mut transaction, ballot_uuid, &form).await {
        return database_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return database_error(error);
    }
    render_template_page(
        &VotePage {
            election_uuid,
            election_name: election.get("name"),
            confirming: false,
            voting_code: "",
            receipt: &receipt,
            debug_bypass,
            tickets: &[],
            council_candidates: &[],
            positions: &[],
            council_seats: 0,
        },
        "Vote complete",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

async fn load_election(
    state: &AppState,
    election_uuid: uuid::Uuid,
) -> Result<sqlx::postgres::PgRow, Response> {
    sqlx::query("SELECT name, status::text AS status, registration_starts_at, registration_ends_at,
                        voter_code_registration_starts_at, voter_code_registration_ends_at,
                        voting_starts_at, voting_ends_at, paused_stage, council_seats, active_revote_id
                 FROM elections WHERE uuid = $1 AND status <> 'draft'")
        .bind(election_uuid).fetch_optional(&state.pool).await.map_err(database_error)?.ok_or_else(|| error_response(StatusCode::NOT_FOUND, &ErrorPage::new("Election not found", "", "election-not-found-page")))
}

async fn load_choices(
    state: &AppState,
    election_uuid: uuid::Uuid,
) -> Result<BallotChoices, Response> {
    let scope = sqlx::query_scalar::<_, String>("SELECT revotes.contest::text FROM elections JOIN election_revotes revotes ON revotes.uuid = elections.active_revote_id WHERE elections.uuid = $1 AND revotes.mode IN ('selected_contest', 'runoff')")
        .bind(election_uuid).fetch_optional(&state.pool).await.map_err(database_error)?;
    let active_revote = sqlx::query_scalar::<_, Option<uuid::Uuid>>(
        "SELECT active_revote_id FROM elections WHERE uuid = $1",
    ).bind(election_uuid).fetch_one(&state.pool).await.map_err(database_error)?;
    let ticket_rows = sqlx::query(
        "SELECT presidential_tickets.uuid, president.citizen_id AS president_uuid, president.election_display_name AS president_name,
                vice_president.citizen_id AS vice_president_uuid, vice_president.election_display_name AS vice_president_name, president.party
         FROM presidential_tickets JOIN candidates president ON president.uuid = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1 AND ($2::text IS NULL OR $2 = 'president')
           AND (($3::uuid IS NULL AND presidential_tickets.status = 'active' AND president.status = 'active' AND vice_president.status = 'active')
                OR EXISTS (SELECT 1 FROM election_revote_tickets WHERE revote_id = $3 AND ticket_id = presidential_tickets.uuid))
         ORDER BY president.election_display_name"
    ).bind(election_uuid).bind(scope.as_deref()).bind(active_revote).fetch_all(&state.pool).await.map_err(database_error)?;
    let candidate_rows = sqlx::query(
        "SELECT uuid, citizen_id, election_display_name, party, position::text AS position FROM candidates
         WHERE election_id = $1 AND ($2::text IS NULL OR position::text = $2) AND position NOT IN ('president', 'vice_president')
           AND (($3::uuid IS NULL AND status = 'active') OR EXISTS
                (SELECT 1 FROM election_revote_candidates choices WHERE choices.revote_id = $3 AND choices.candidate_id = candidates.uuid AND choices.contest = candidates.position))
         ORDER BY position::text, election_display_name"
    ).bind(election_uuid).bind(scope.as_deref()).bind(active_revote).fetch_all(&state.pool).await.map_err(database_error)?;
    let applicable = sqlx::query_scalar::<_, String>("SELECT position::text FROM election_positions WHERE election_id = $1 AND ($2::text IS NULL OR position::text = $2) ORDER BY position::text")
        .bind(election_uuid).bind(scope.as_deref()).fetch_all(&state.pool).await.map_err(database_error)?;
    let tickets = ticket_rows
        .into_iter()
        .map(|row| TicketChoice {
            uuid: row.get("uuid"),
            president_uuid: row.get("president_uuid"),
            president_name: row.get("president_name"),
            vice_president_uuid: row.get("vice_president_uuid"),
            vice_president_name: row.get("vice_president_name"),
            party: row.get("party"),
        })
        .collect();
    let mut council_candidates = Vec::new();
    let mut positions: Vec<PositionChoice> = applicable
        .into_iter()
        .filter(|position| {
            !matches!(
                position.as_str(),
                "president" | "vice_president" | "council"
            )
        })
        .map(|position| PositionChoice {
            label: crate::pages::election_lifecycle::position_label(&position).to_string(),
            position,
            candidates: Vec::new(),
        })
        .collect();
    for row in candidate_rows {
        let position: String = row.get("position");
        let party: String = row.get("party");
        let candidate = CandidateChoice {
            uuid: row.get("uuid"),
            user_uuid: row.get("citizen_id"),
            name: row.get("election_display_name"),
            party: party.clone(),
        };
        if position == "council" {
            council_candidates.push(candidate);
        } else if let Some(group) = positions
            .iter_mut()
            .find(|group| group.position == position)
        {
            group.candidates.push(candidate);
        } else {
            positions.push(PositionChoice {
                label: crate::pages::election_lifecycle::position_label(&position).to_string(),
                position,
                candidates: vec![candidate],
            });
        }
    }
    Ok(BallotChoices {
        tickets,
        council_candidates,
        positions,
        council_seats: sqlx::query_scalar(
            "SELECT COALESCE(revotes.unresolved_seats, elections.council_seats)
             FROM elections LEFT JOIN election_revotes revotes ON revotes.uuid = elections.active_revote_id
             WHERE elections.uuid = $1",
        ).bind(election_uuid).fetch_one(&state.pool).await.map_err(database_error)?,
    })
}

async fn validate_choices(
    state: &AppState,
    election_uuid: uuid::Uuid,
    form: &BallotForm,
) -> Result<(), &'static str> {
    let choices = load_choices(state, election_uuid)
        .await
        .map_err(|_| "Candidates could not be loaded.")?;
    if form.presidential_ticket.len() != form.presidential_ranking.len() { return Err("The presidential ranking is malformed."); }
    let mut presidential_ranks = HashSet::new();
    for (&ticket, &ranking) in form.presidential_ticket.iter().zip(&form.presidential_ranking) {
        if ranking == 0 { continue; }
        if ranking < 0 || !choices.tickets.iter().any(|choice| choice.uuid == ticket) || !presidential_ranks.insert(ranking) { return Err("Presidential rankings are invalid."); }
    }
    if presidential_ranks.iter().copied().max().unwrap_or(0) as usize != presidential_ranks.len() {
        return Err("Presidential rankings must start at 1 and have no gaps.");
    }
    let seats = choices.council_seats;
    let council = form.council_candidate.iter().collect::<HashSet<_>>();
    if form.abstain_position.iter().any(|position| position == "council") && !council.is_empty() {
        return Err("Council abstention cannot be combined with candidate choices.");
    }
    if council.len() != form.council_candidate.len() || council.len() > seats as usize {
        return Err("Choose no more council candidates than there are seats.");
    }
    if !council.iter().all(|candidate| choices.council_candidates.iter().any(|available| available.uuid == **candidate)) {
        return Err("A selected council candidate is not available.");
    }
    let mut positions = HashSet::new();
    for candidate in &form.candidate_id {
        let Some(group) = choices.positions.iter().find(|group| {
            group
                .candidates
                .iter()
                .any(|available| available.uuid == *candidate)
        }) else {
            return Err("A selected candidate is not available.");
        };
        if !positions.insert(group.position.as_str()) {
            return Err("Only one candidate may be selected per position.");
        }
    }
    if form.abstain_position.iter().any(|position| positions.contains(position.as_str())) {
        return Err("Abstention cannot be combined with a candidate choice.");
    }
    if !form.abstain_position.iter().all(|position| position == "council" || choices.positions.iter().any(|group| group.position == *position)) {
        return Err("An abstention is not valid for this ballot.");
    }
    Ok(())
}

async fn store_choices(
    transaction: &mut Transaction<'_, Postgres>,
    ballot_uuid: uuid::Uuid,
    form: &BallotForm,
) -> Result<(), sqlx::Error> {
    for (&ticket, &ranking) in form.presidential_ticket.iter().zip(&form.presidential_ranking) {
        if ranking == 0 { continue; }
        sqlx::query("INSERT INTO presidential_votes (ballot_uuid, ticket_id, ranking) VALUES ($1, $2, $3)")
            .bind(ballot_uuid).bind(ticket).bind(ranking).execute(&mut **transaction).await?;
    }
    for candidate in &form.council_candidate {
        sqlx::query("INSERT INTO candidate_votes (ballot_uuid, candidate_id, position, ranking) VALUES ($1, $2, 'council', 1)")
            .bind(ballot_uuid).bind(candidate).execute(&mut **transaction).await?;
    }
    for candidate in &form.candidate_id {
        sqlx::query("INSERT INTO candidate_votes (ballot_uuid, candidate_id, position, ranking) SELECT $1, uuid, position, 1 FROM candidates WHERE uuid = $2")
            .bind(ballot_uuid).bind(candidate).execute(&mut **transaction).await?;
    }
    Ok(())
}

fn effective_stage(row: &sqlx::postgres::PgRow) -> String {
    timeline(
        row.get("status"),
        row.get("registration_starts_at"),
        row.get("registration_ends_at"),
        row.get("voting_starts_at"),
        row.get("voting_ends_at"),
        row.get::<Option<String>, _>("paused_stage").as_deref(),
        chrono::Utc::now(),
    )
    .stage
}

fn code_requests_open(row: &sqlx::postgres::PgRow) -> bool {
    let starts = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("voter_code_registration_starts_at")
        .or_else(|| row.get("registration_starts_at"));
    let ends = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("voter_code_registration_ends_at")
        .or_else(|| row.get("voting_ends_at"));
    matches!(effective_stage(row).as_str(), "registration" | "upcoming" | "voting")
        && starts.is_some_and(|starts| chrono::Utc::now() >= starts)
        && ends.is_some_and(|ends| chrono::Utc::now() < ends)
}

pub async fn ensure_snapshot(state: &AppState, election_uuid: uuid::Uuid) -> Result<(), Response> {
    let mut transaction = state.pool.begin().await.map_err(database_error)?;
    let claimed = sqlx::query("UPDATE elections SET eligibility_snapshotted_at = CURRENT_TIMESTAMP WHERE uuid = $1 AND eligibility_snapshotted_at IS NULL RETURNING uuid")
        .bind(election_uuid).fetch_optional(&mut *transaction).await.map_err(database_error)?.is_some();
    if claimed {
        sqlx::query("INSERT INTO election_eligibility (election_id, citizen_id) SELECT $1, census_entries.citizen_uuid FROM censuses JOIN census_entries ON census_entries.census_uuid = censuses.uuid WHERE censuses.active = TRUE AND census_entries.status = 'filled_out'")
            .bind(election_uuid).execute(&mut *transaction).await.map_err(database_error)?;
    }
    transaction.commit().await.map_err(database_error)
}

async fn find_code(state: &AppState, election_uuid: uuid::Uuid, revote_id: Option<uuid::Uuid>, value: &str) -> Option<uuid::Uuid> {
    let value = normalise_code(value);
    let lookup_hash = Sha256::digest(value.as_bytes()).to_vec();
    sqlx::query("SELECT uuid, code_hash FROM voting_codes WHERE election_id = $1 AND revote_id IS NOT DISTINCT FROM $2 AND code_lookup_hash = $3 AND used = FALSE")
        .bind(election_uuid)
        .bind(revote_id)
        .bind(lookup_hash)
        .fetch_optional(&state.pool)
        .await
        .ok()?
        .filter(|row| verify_hash(&value, row.get("code_hash")))
        .map(|row| row.get("uuid"))
}

fn random_code() -> String {
    const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let random = uuid::Uuid::new_v4();
    let value: String = random.as_bytes()[..8]
        .iter()
        .map(|byte| ALPHANUMERIC[*byte as usize % ALPHANUMERIC.len()] as char)
        .collect();
    format!("{}-{}", &value[..4], &value[4..])
}
fn normalise_code(value: &str) -> String {
    value.trim().to_uppercase()
}
fn expensive_hash(value: &str, salt: &str) -> String {
    let mut bytes = format!("{salt}:{value}").into_bytes();
    for _ in 0..100_000 {
        bytes = Sha256::digest(&bytes).to_vec();
    }
    format!("{salt}:{}", hex(&bytes))
}
fn verify_hash(value: &str, stored: String) -> bool {
    let Some((salt, _)) = stored.split_once(':') else {
        return false;
    };
    expensive_hash(value, salt) == stored
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn invalid_code() -> Response {
    bad_request("The voter code could not be accepted.")
}
fn bad_request(message: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new("Could not continue", message, "voting-error-page"),
    )
}
fn database_error(error: sqlx::Error) -> Response {
    error!(?error, "Voting database request failed");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new("Could not continue", "", "voting-error-page"),
    )
}
