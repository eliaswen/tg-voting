use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
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
        "INSERT INTO voting_codes (election_id, code_hash, code_lookup_hash) VALUES ($1, $2, $3)",
    )
    .bind(election_uuid)
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
    render_template_page(
        &VotePage {
            election_uuid,
            election_name: election.get("name"),
            confirming: false,
            voting_code: "",
            receipt: "",
            debug_bypass: state.app_mode == 0,
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
            && find_code(&state, election_uuid, &form.voting_code)
                .await
                .is_none())
    {
        return invalid_code();
    }
    render_template_page(
        &VotePage {
            election_uuid,
            election_name: election.get("name"),
            confirming: true,
            voting_code: &normalise_code(&form.voting_code),
            receipt: "",
            debug_bypass,
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
    Form(form): Form<VotingCodeForm>,
) -> Response {
    let election = match load_election(&state, election_uuid).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if effective_stage(&election) != "voting" {
        return invalid_code();
    }
    let debug_bypass = state.app_mode == 0;
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return database_error(error),
    };
    let normalised = normalise_code(&form.voting_code);
    if debug_bypass && normalised.is_empty() {
        let receipt = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
        let authorization_hash = Sha256::digest(uuid::Uuid::new_v4().as_bytes()).to_vec();
        if let Err(error) = sqlx::query("INSERT INTO ballots (election_id, authorization_hash, receipt_number) VALUES ($1, $2, $3)").bind(election_uuid).bind(authorization_hash).bind(&receipt).execute(&mut *transaction).await { return database_error(error); }
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
            },
            "Vote complete",
            jar,
            &state.pool,
        )
        .await
        .into_response();
    }
    let lookup_hash = Sha256::digest(normalised.as_bytes()).to_vec();
    let code = match sqlx::query("SELECT uuid, code_hash FROM voting_codes WHERE election_id = $1 AND code_lookup_hash = $2 AND used = FALSE FOR UPDATE").bind(election_uuid).bind(lookup_hash).fetch_optional(&mut *transaction).await { Ok(Some(code)) if verify_hash(&normalised, code.get("code_hash")) => code, _ => {
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
    if let Err(error) = sqlx::query("INSERT INTO ballots (election_id, authorization_hash, voting_code_uuid, receipt_number) VALUES ($1, $2, $3, $4)").bind(election_uuid).bind(authorization_hash).bind(code_uuid).bind(&receipt).execute(&mut *transaction).await { return database_error(error); }
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
    sqlx::query("SELECT name, status::text AS status, registration_starts_at, registration_ends_at, voting_starts_at, voting_ends_at, paused_stage FROM elections WHERE uuid = $1 AND status <> 'draft'")
        .bind(election_uuid).fetch_optional(&state.pool).await.map_err(database_error)?.ok_or_else(|| error_response(StatusCode::NOT_FOUND, &ErrorPage::new("Election not found", "", "election-not-found-page")))
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
    matches!(
        effective_stage(row).as_str(),
        "registration" | "upcoming" | "voting"
    ) && chrono::Utc::now()
        >= row
            .get::<Option<chrono::DateTime<chrono::Utc>>, _>("registration_starts_at")
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC)
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

async fn find_code(state: &AppState, election_uuid: uuid::Uuid, value: &str) -> Option<uuid::Uuid> {
    let value = normalise_code(value);
    let lookup_hash = Sha256::digest(value.as_bytes()).to_vec();
    sqlx::query("SELECT uuid, code_hash FROM voting_codes WHERE election_id = $1 AND code_lookup_hash = $2 AND used = FALSE")
        .bind(election_uuid)
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
