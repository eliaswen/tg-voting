use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use tracing::{debug, error, info, trace, warn};

use crate::error_handling::{ErrorPage, error_response};
use crate::pages::auth::{AuthenticatedCitizen, require_election_manager};
use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Deserialize)]
pub struct StatusForm {
    status: String,
    reason: String,
    #[serde(default)]
    expected_resume_at: String,
    #[serde(default)]
    registration_starts_at: String,
    #[serde(default)]
    registration_ends_at: String,
    #[serde(default)]
    voting_starts_at: String,
    #[serde(default)]
    voting_ends_at: String,
    #[serde(default)]
    debug: bool,
}

#[derive(Deserialize, Default)]
pub struct ChangeSearch {
    #[serde(default)]
    q: String,
}

#[derive(Deserialize)]
pub struct CouncilCandidateForm {
    election_display_name: String,
    party: String,
    status: String,
    reason: String,
}

#[derive(Deserialize)]
pub struct PresidentialTicketForm {
    president_display_name: String,
    president_party: String,
    vice_president_citizen_id: uuid::Uuid,
    vice_president_display_name: String,
    vice_president_party: String,
    message_1: String,
    message_2: String,
    message_3: String,
    message_4: String,
    message_5: String,
    status: String,
    reason: String,
}

#[derive(Template)]
#[template(path = "manage-election-status/status.html")]
struct ElectionStatusPage<'a> {
    election_uuid: uuid::Uuid,
    season: i32,
    name: String,
    status: String,
    registration_starts_at: String,
    registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    maximum_council_choices: i32,
    statuses: &'a [StatusOption],
    debug_mode: bool,
}

struct StatusOption {
    value: &'static str,
    selected: bool,
}

struct ElectionChange {
    changed_at: String,
    actor: String,
    target: String,
    previous: String,
    new: String,
    reason: String,
}

#[derive(Template)]
#[template(path = "manage-election-status/changes.html")]
struct ElectionChangesPage<'a> {
    election_name: &'a str,
    election_uuid: uuid::Uuid,
    search_query: &'a str,
    changes: &'a [ElectionChange],
}

struct ManagedVicePresidentOption {
    id: uuid::Uuid,
    selected: bool,
    label: String,
}

struct ManagedPresidentialTicket {
    uuid: uuid::Uuid,
    president_name: String,
    president_party: String,
    vice_president_options: Vec<ManagedVicePresidentOption>,
    vice_president_name: String,
    vice_president_party: String,
    messages: Vec<String>,
    status: String,
}

impl ManagedPresidentialTicket {
    fn is_status(&self, status: &str) -> bool {
        self.status == status
    }
}

struct ManagedCouncilCandidate {
    uuid: uuid::Uuid,
    name: String,
    party: String,
    status: String,
    position: String,
}

impl ManagedCouncilCandidate {
    fn is_status(&self, status: &str) -> bool {
        self.status == status
    }
}

struct FormerVicePresident {
    name: String,
    party: String,
    status: String,
}

#[derive(Template)]
#[template(path = "manage-election-status/candidates.html")]
struct CandidateManagementPage<'a> {
    election_uuid: uuid::Uuid,
    season: i32,
    name: String,
    status: String,
    presidential: &'a [ManagedPresidentialTicket],
    council: &'a [ManagedCouncilCandidate],
    former_vice_presidents: &'a [FormerVicePresident],
    registration_statuses: &'a [&'static str],
}

pub async fn get_manage_election_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    render_election_management(state, jar, election_uuid, false).await
}

pub async fn get_manage_election_candidates(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    render_election_management(state, jar, election_uuid, true).await
}

async fn render_election_management(
    state: AppState,
    jar: CookieJar,
    election_uuid: uuid::Uuid,
    candidates_page: bool,
) -> Response {
    trace!(%election_uuid, "Handling election status management page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }

    let timezone = crate::render::timezone(&jar);
    let election_query = "SELECT uuid AS id, season, name, status::text AS status,
                COALESCE(to_char(registration_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS registration_starts_at,
                COALESCE(to_char(registration_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS registration_ends_at,
                registration_starts_at AS registration_start_raw, registration_ends_at AS registration_end_raw,
                voting_starts_at AS voting_start_raw, voting_ends_at AS voting_end_raw, paused_stage,
                COALESCE(to_char(voting_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS voting_starts_at,
                COALESCE(to_char(voting_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS voting_ends_at,
                maximum_council_choices
         FROM elections WHERE uuid = $1".replace("Europe/Paris", &timezone);
    let election = sqlx::query(sqlx::AssertSqlSafe(election_query.as_str()))
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await;
    let election = match election {
        Ok(Some(election)) => election,
        Ok(None) => return not_found(),
        Err(error) => {
            error!(?error, "Failed to retrieve election status page");
            return server_error();
        }
    };

    let election_id: uuid::Uuid = election.get("id");
    let status: String = election.get("status");
    debug!(%election_uuid, %election_id, %status, "Retrieved election status management context");
    if !candidates_page {
        let effective = crate::pages::election_lifecycle::timeline(
            &status,
            election.get("registration_start_raw"),
            election.get("registration_end_raw"),
            election.get("voting_start_raw"),
            election.get("voting_end_raw"),
            election.get::<Option<String>, _>("paused_stage").as_deref(),
            chrono::Utc::now(),
        );
        let allowed: Vec<&'static str> = match effective.stage.as_str() {
            "draft" => vec!["upcoming"],
            "counting" => vec!["closed", "paused", "canceled"],
            "closed" => vec!["certified"],
            "paused" => vec!["upcoming", "canceled"],
            "upcoming" | "registration" | "voting" => vec!["paused", "canceled"],
            _ => Vec::new(),
        };
        let statuses: Vec<StatusOption> = allowed
            .into_iter()
            .map(|value| StatusOption {
                selected: value == status,
                value,
            })
            .collect();
        let page = ElectionStatusPage {
            election_uuid,
            season: election.get("season"),
            name: election.get("name"),
            status: effective.stage_label,
            registration_starts_at: election.get("registration_starts_at"),
            registration_ends_at: election.get("registration_ends_at"),
            voting_starts_at: election.get("voting_starts_at"),
            voting_ends_at: election.get("voting_ends_at"),
            maximum_council_choices: election.get("maximum_council_choices"),
            statuses: &statuses,
            debug_mode: state.app_mode == 0,
        };
        return render_template_page(&page, "Election status", jar, &state.pool)
            .await
            .into_response();
    }

    let tickets = match sqlx::query(
        "SELECT presidential_tickets.uuid, presidential_tickets.status::text AS status,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.citizen_id AS vice_president_citizen_id,
                vice_president.election_display_name AS vice_president_name,
                vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN candidates president ON president.uuid = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1
         ORDER BY president.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await {
        Ok(tickets) => {
            debug!(%election_uuid, ticket_count = tickets.len(), "Retrieved presidential tickets for management");
            tickets
        }
        Err(error) => {
            error!(?error, "Failed to retrieve presidential tickets");
            return server_error();
        }
    };

    let mut presidential_forms = Vec::new();
    for ticket in tickets {
        let ticket_uuid: uuid::Uuid = ticket.get("uuid");
        trace!(%election_uuid, %ticket_uuid, "Rendering presidential ticket management form");
        let current_vp: uuid::Uuid = ticket.get("vice_president_citizen_id");
        let vice_president_options =
            match eligible_vp_options(&state, election_id, current_vp).await {
                Ok(options) => options,
                Err(response) => return response,
            };
        let messages = match ticket_messages(&state, ticket_uuid).await {
            Ok(messages) => messages,
            Err(response) => return response,
        };
        presidential_forms.push(ManagedPresidentialTicket {
            uuid: ticket_uuid,
            president_name: ticket.get("president_name"),
            president_party: ticket.get("president_party"),
            vice_president_options,
            vice_president_name: ticket.get("vice_president_name"),
            vice_president_party: ticket.get("vice_president_party"),
            messages,
            status: ticket.get("status"),
        });
    }

    let council = match sqlx::query(
        "SELECT uuid, election_display_name, party, status::text AS status, position::text AS position
         FROM candidates
         WHERE election_id = $1 AND position NOT IN ('president', 'vice_president')
         ORDER BY position::text, election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(council) => {
            debug!(%election_uuid, candidate_count = council.len(), "Retrieved council candidates for management");
            council
        }
        Err(error) => {
            error!(?error, "Failed to retrieve council candidates");
            return server_error();
        }
    };

    let mut council_forms = Vec::new();
    for candidate in council {
        let candidate_uuid: uuid::Uuid = candidate.get("uuid");
        trace!(%election_uuid, %candidate_uuid, "Rendering council candidate management form");
        council_forms.push(ManagedCouncilCandidate {
            uuid: candidate_uuid,
            name: candidate.get("election_display_name"),
            party: candidate.get("party"),
            status: candidate.get("status"),
            position: crate::pages::election_lifecycle::position_label(
                &candidate.get::<String, _>("position"),
            )
            .to_string(),
        });
    }

    let former_vice_presidents = match sqlx::query(
        "SELECT candidates.election_display_name, candidates.party, candidates.status::text AS status
         FROM candidates
         WHERE candidates.election_id = $1
         AND candidates.position = 'vice_president'
         AND NOT EXISTS (
             SELECT 1 FROM presidential_tickets
             WHERE presidential_tickets.vice_president_candidate_id = candidates.uuid
         )
         ORDER BY candidates.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(candidates) => {
            debug!(%election_uuid, candidate_count = candidates.len(), "Retrieved former vice presidents");
            candidates
        }
        Err(error) => {
            error!(?error, "Failed to retrieve former vice presidents");
            return server_error();
        }
    };
    let mut former_vice_president_items = Vec::new();
    for candidate in former_vice_presidents {
        former_vice_president_items.push(FormerVicePresident {
            name: candidate.get("election_display_name"),
            party: candidate.get("party"),
            status: candidate.get("status"),
        });
    }

    let page = CandidateManagementPage {
        election_uuid,
        season: election.get("season"),
        name: election.get("name"),
        status,
        presidential: &presidential_forms,
        council: &council_forms,
        former_vice_presidents: &former_vice_president_items,
        registration_statuses: &["active", "withdrawn", "invalidated"],
    };
    trace!(%election_uuid, candidates_page, "Rendering election management page");
    render_template_page(&page, "Manage candidates", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_election_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<StatusForm>,
) -> Response {
    trace!(%election_uuid, requested_status = %form.status, "Handling election status change");
    let timezone = crate::render::timezone(&jar);
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if form.debug {
        if state.app_mode != 0 {
            return not_found();
        }
        return run_inline_debug_action(&state, election_uuid, &form.status).await;
    }
    if !valid_election_status(&form.status) || form.reason.trim().is_empty() {
        warn!(%election_uuid, actor_citizen_id = actor.id, requested_status = %form.status, reason_present = !form.reason.trim().is_empty(), "Rejected invalid election status change");
        return bad_request("A valid status and reason are required.");
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let previous_row = sqlx::query(
        "SELECT status::text AS status, registration_starts_at, registration_ends_at, voting_starts_at, voting_ends_at, paused_stage FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let previous_row = match previous_row {
        Ok(Some(previous)) => previous,
        Ok(None) => return not_found(),
        Err(error) => return transaction_error(error),
    };
    let previous: String = previous_row.get("status");
    let effective = crate::pages::election_lifecycle::timeline(
        &previous,
        previous_row.get("registration_starts_at"),
        previous_row.get("registration_ends_at"),
        previous_row.get("voting_starts_at"),
        previous_row.get("voting_ends_at"),
        previous_row
            .get::<Option<String>, _>("paused_stage")
            .as_deref(),
        chrono::Utc::now(),
    );
    let allowed = match effective.stage.as_str() {
        "draft" => form.status == "upcoming",
        "upcoming" | "registration" | "voting" => {
            matches!(form.status.as_str(), "paused" | "canceled")
        }
        "counting" => matches!(form.status.as_str(), "closed" | "paused" | "canceled"),
        "closed" => form.status == "certified",
        "paused" => matches!(form.status.as_str(), "upcoming" | "canceled"),
        _ => false,
    };
    if !allowed {
        return bad_request("That stage change is not allowed from the current stage.");
    }
    if previous == "draft"
        && [
            previous_row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("registration_starts_at"),
            previous_row.get("registration_ends_at"),
            previous_row.get("voting_starts_at"),
            previous_row.get("voting_ends_at"),
        ]
        .iter()
        .any(Option::is_none)
    {
        return bad_request("Set the complete election schedule before publishing.");
    }
    debug!(%election_uuid, actor_citizen_id = actor.id, previous_status = %previous, requested_status = %form.status, "Loaded election status change");
    let saved_status = if previous == "paused" && form.status == "upcoming" {
        "upcoming"
    } else {
        &form.status
    };
    if previous == "paused" && form.status == "upcoming" {
        let parse = |value: &str| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M");
        match (
            parse(&form.registration_starts_at),
            parse(&form.registration_ends_at),
            parse(&form.voting_starts_at),
            parse(&form.voting_ends_at),
        ) {
            (Ok(a), Ok(b), Ok(c), Ok(d)) if a < b && b <= c && c < d => {}
            _ => {
                return bad_request("A complete chronological schedule is required when resuming.");
            }
        }
        let resume_query = "UPDATE elections SET registration_starts_at = $1::timestamp AT TIME ZONE 'Europe/Paris', registration_ends_at = $2::timestamp AT TIME ZONE 'Europe/Paris', voting_starts_at = $3::timestamp AT TIME ZONE 'Europe/Paris', voting_ends_at = $4::timestamp AT TIME ZONE 'Europe/Paris' WHERE uuid = $5".replace("Europe/Paris", &timezone);
        if let Err(error) = sqlx::query(sqlx::AssertSqlSafe(resume_query.as_str()))
            .bind(&form.registration_starts_at)
            .bind(&form.registration_ends_at)
            .bind(&form.voting_starts_at)
            .bind(&form.voting_ends_at)
            .bind(election_uuid)
            .execute(&mut *transaction)
            .await
        {
            return transaction_error(error);
        }
    }
    let expected_resume = if form.status == "paused" {
        match chrono::NaiveDateTime::parse_from_str(&form.expected_resume_at, "%Y-%m-%dT%H:%M") {
            Ok(_) => Some(form.expected_resume_at.as_str()),
            Err(_) => return bad_request("An expected resume date is required when pausing."),
        }
    } else {
        None
    };
    let status_query = "UPDATE elections SET status = $1::election_status, published_at = CASE WHEN $1 = 'upcoming' AND published_at IS NULL THEN CURRENT_TIMESTAMP ELSE published_at END, paused_at = CASE WHEN $1 = 'paused' THEN CURRENT_TIMESTAMP ELSE NULL END, paused_stage = CASE WHEN $1 = 'paused' THEN $3 ELSE NULL END, expected_resume_at = CASE WHEN $4::text IS NULL THEN NULL ELSE $4::timestamp AT TIME ZONE 'Europe/Paris' END WHERE uuid = $2".replace("Europe/Paris", &timezone);
    if let Err(error) = sqlx::query(sqlx::AssertSqlSafe(status_query.as_str()))
        .bind(saved_status)
        .bind(election_uuid)
        .bind(&effective.stage)
        .bind(expected_resume)
        .execute(&mut *transaction)
        .await
    {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "election status",
        election_uuid,
        &previous,
        &form.status,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, actor_citizen_id = actor.id, previous_status = %previous, new_status = %form.status, "Changed election status");
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

async fn run_inline_debug_action(
    state: &AppState,
    election_uuid: uuid::Uuid,
    action: &str,
) -> Response {
    let result = match action {
        "draft" => sqlx::query("UPDATE elections SET status = 'draft' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "upcoming" => sqlx::query("UPDATE elections SET status = 'upcoming', published_at = COALESCE(published_at, CURRENT_TIMESTAMP), registration_starts_at = CURRENT_TIMESTAMP + INTERVAL '2 minutes', registration_ends_at = CURRENT_TIMESTAMP + INTERVAL '4 minutes', voting_starts_at = CURRENT_TIMESTAMP + INTERVAL '6 minutes', voting_ends_at = CURRENT_TIMESTAMP + INTERVAL '8 minutes' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "registration" => sqlx::query("UPDATE elections SET status = 'upcoming', published_at = COALESCE(published_at, CURRENT_TIMESTAMP), registration_starts_at = CURRENT_TIMESTAMP - INTERVAL '1 minute', registration_ends_at = CURRENT_TIMESTAMP + INTERVAL '2 minutes', voting_starts_at = CURRENT_TIMESTAMP + INTERVAL '4 minutes', voting_ends_at = CURRENT_TIMESTAMP + INTERVAL '6 minutes' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "voting" => sqlx::query("UPDATE elections SET status = 'upcoming', published_at = COALESCE(published_at, CURRENT_TIMESTAMP), registration_starts_at = CURRENT_TIMESTAMP - INTERVAL '6 minutes', registration_ends_at = CURRENT_TIMESTAMP - INTERVAL '4 minutes', voting_starts_at = CURRENT_TIMESTAMP - INTERVAL '1 minute', voting_ends_at = CURRENT_TIMESTAMP + INTERVAL '2 minutes' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "counting" => sqlx::query("UPDATE elections SET status = 'upcoming', published_at = COALESCE(published_at, CURRENT_TIMESTAMP), registration_starts_at = CURRENT_TIMESTAMP - INTERVAL '8 minutes', registration_ends_at = CURRENT_TIMESTAMP - INTERVAL '6 minutes', voting_starts_at = CURRENT_TIMESTAMP - INTERVAL '4 minutes', voting_ends_at = CURRENT_TIMESTAMP - INTERVAL '1 minute' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "closed" | "certified" | "canceled" => sqlx::query("UPDATE elections SET status = $1::election_status WHERE uuid = $2").bind(action).bind(election_uuid).execute(&state.pool).await,
        "schedule" => sqlx::query("UPDATE elections SET status = 'upcoming', published_at = COALESCE(published_at, CURRENT_TIMESTAMP), registration_starts_at = CURRENT_TIMESTAMP + INTERVAL '1 minute', registration_ends_at = CURRENT_TIMESTAMP + INTERVAL '3 minutes', voting_starts_at = CURRENT_TIMESTAMP + INTERVAL '5 minutes', voting_ends_at = CURRENT_TIMESTAMP + INTERVAL '8 minutes' WHERE uuid = $1").bind(election_uuid).execute(&state.pool).await,
        "users" => sqlx::query("INSERT INTO citizens (citizen_id) SELECT lpad((floor(random() * 1000000))::int::text, 6, '0') FROM generate_series(1, 20) ON CONFLICT (citizen_id) DO NOTHING").execute(&state.pool).await,
        "candidates" => sqlx::query("INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party) SELECT $1, citizens.uuid, 'council', 'garbage-' || substr(gen_random_uuid()::text, 1, 8), 'garbage-party' FROM citizens ORDER BY random() LIMIT 20 ON CONFLICT DO NOTHING").bind(election_uuid).execute(&state.pool).await,
        _ => return bad_request("Unknown debug action."),
    };
    match result {
        Ok(_) => Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response(),
        Err(error) => transaction_error(error),
    }
}

pub async fn post_manage_council_candidate(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, candidate_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<CouncilCandidateForm>,
) -> Response {
    trace!(%election_uuid, %candidate_uuid, requested_status = %form.status, "Handling council candidate management update");
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(message) = validate_candidate_fields(
        &form.election_display_name,
        &form.party,
        &form.status,
        &form.reason,
    ) {
        warn!(%election_uuid, %candidate_uuid, actor_citizen_id = actor.id, validation_error = message, "Rejected invalid council candidate update");
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let candidate = sqlx::query(
        "SELECT candidates.election_display_name, candidates.party, candidates.status::text AS status
         FROM candidates JOIN elections ON elections.uuid = candidates.election_id
         WHERE elections.uuid = $1 AND candidates.uuid = $2 AND candidates.position NOT IN ('president', 'vice_president')
         FOR UPDATE OF candidates",
    )
    .bind(election_uuid)
    .bind(candidate_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let candidate = match candidate {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            debug!(%election_uuid, %candidate_uuid, "Council candidate management target was not found");
            return not_found();
        }
        Err(error) => return transaction_error(error),
    };
    let previous = format!(
        "{}; party {}; status {}",
        candidate.get::<String, _>("election_display_name"),
        candidate.get::<String, _>("party"),
        candidate.get::<String, _>("status"),
    );
    let new_value = format!(
        "{}; party {}; status {}",
        form.election_display_name.trim(),
        form.party.trim(),
        form.status,
    );
    if let Err(error) = sqlx::query(
        "UPDATE candidates SET election_display_name = $1, party = $2, status = $3::registration_status
         WHERE uuid = $4",
    )
    .bind(form.election_display_name.trim())
    .bind(form.party.trim())
    .bind(&form.status)
    .bind(candidate_uuid)
    .execute(&mut *transaction)
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "council candidate",
        candidate_uuid,
        &previous,
        &new_value,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, %candidate_uuid, actor_citizen_id = actor.id, new_status = %form.status, "Updated council candidate");
    Redirect::to(&format!("/manage/elections/{election_uuid}/candidates")).into_response()
}

pub async fn post_manage_presidential_ticket(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, ticket_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<PresidentialTicketForm>,
) -> Response {
    trace!(%election_uuid, %ticket_uuid, requested_status = %form.status, "Handling presidential ticket management update");
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(message) = validate_presidential_form(&form) {
        warn!(%election_uuid, %ticket_uuid, actor_citizen_id = actor.id, validation_error = message, "Rejected invalid presidential ticket update");
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let ticket = sqlx::query(
        "SELECT presidential_tickets.uuid AS id, presidential_tickets.status::text AS status,
                president.uuid AS president_id, president.citizen_id AS president_citizen_id,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.uuid AS vice_president_id, vice_president.citizen_id AS old_vp_citizen_id,
                vice_president.election_display_name AS vice_president_name, vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN elections ON elections.uuid = presidential_tickets.election_id
         JOIN candidates president ON president.uuid = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
         WHERE elections.uuid = $1 AND presidential_tickets.uuid = $2
         FOR UPDATE OF presidential_tickets, president, vice_president",
    )
    .bind(election_uuid)
    .bind(ticket_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let ticket = match ticket {
        Ok(Some(ticket)) => ticket,
        Ok(None) => {
            debug!(%election_uuid, %ticket_uuid, "Presidential ticket management target was not found");
            return not_found();
        }
        Err(error) => return transaction_error(error),
    };
    let president_citizen_id: uuid::Uuid = ticket.get("president_citizen_id");
    if form.vice_president_citizen_id == president_citizen_id {
        return bad_request("The president cannot also be the vice president.");
    }
    let old_vp_citizen_id: uuid::Uuid = ticket.get("old_vp_citizen_id");
    let new_vp_id = if form.vice_president_citizen_id == old_vp_citizen_id {
        trace!(%ticket_uuid, vice_president_citizen_id = %old_vp_citizen_id, "Keeping presidential ticket vice president");
        ticket.get::<uuid::Uuid, _>("vice_president_id")
    } else {
        debug!(%ticket_uuid, %old_vp_citizen_id, new_vp_citizen_id = %form.vice_president_citizen_id, "Replacing managed presidential ticket vice president");
        let available = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM citizens
                JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
                JOIN elections ON elections.uuid = $1
                WHERE citizens.uuid = $2 AND citizens.banned = FALSE
                AND NOT EXISTS (
                    SELECT 1 FROM candidates
                    WHERE candidates.election_id = elections.uuid AND candidates.citizen_id = citizens.uuid
                    AND candidates.status = 'active'
                )
            )",
        )
        .bind(election_uuid)
        .bind(form.vice_president_citizen_id)
        .fetch_one(&mut *transaction)
        .await;
        match available {
            Ok(true) => {}
            Ok(false) => {
                warn!(%election_uuid, %ticket_uuid, requested_vp_citizen_id = %form.vice_president_citizen_id, "Requested managed ticket vice president is unavailable");
                return bad_request("That vice president is no longer available.");
            }
            Err(error) => return transaction_error(error),
        }
        match sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
             SELECT uuid, $1, 'vice_president', $2, $3 FROM elections WHERE uuid = $4
             RETURNING uuid",
        )
        .bind(form.vice_president_citizen_id)
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .bind(election_uuid)
        .fetch_one(&mut *transaction)
        .await {
            Ok(id) => id,
            Err(error) => return database_candidate_error(error),
        }
    };

    let previous_messages = match sqlx::query_scalar::<_, String>(
        "SELECT message FROM presidential_ticket_messages WHERE presidential_ticket_id = $1 ORDER BY position",
    )
    .bind(ticket.get::<uuid::Uuid, _>("id"))
    .fetch_all(&mut *transaction)
    .await {
        Ok(messages) => messages.join(" | "),
        Err(error) => return transaction_error(error),
    };
    let previous = format!(
        "{} ({}) and {} ({}); messages [{}]; status {}",
        ticket.get::<String, _>("president_name"),
        ticket.get::<String, _>("president_party"),
        ticket.get::<String, _>("vice_president_name"),
        ticket.get::<String, _>("vice_president_party"),
        previous_messages,
        ticket.get::<String, _>("status"),
    );
    let messages = form_messages(&form);
    let new_value = format!(
        "{} ({}) and {} ({}); messages [{}]; status {}",
        form.president_display_name.trim(),
        form.president_party.trim(),
        form.vice_president_display_name.trim(),
        form.vice_president_party.trim(),
        messages.join(" | "),
        form.status,
    );

    if let Err(error) =
        sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE uuid = $3")
            .bind(form.president_display_name.trim())
            .bind(form.president_party.trim())
            .bind(ticket.get::<uuid::Uuid, _>("president_id"))
            .execute(&mut *transaction)
            .await
    {
        return transaction_error(error);
    }
    if let Err(error) =
        sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE uuid = $3")
            .bind(form.vice_president_display_name.trim())
            .bind(form.vice_president_party.trim())
            .bind(new_vp_id)
            .execute(&mut *transaction)
            .await
    {
        return transaction_error(error);
    }
    if let Err(error) = sqlx::query(
        "UPDATE presidential_tickets SET vice_president_candidate_id = $1, status = $2::registration_status WHERE uuid = $3",
    )
    .bind(new_vp_id)
    .bind(&form.status)
    .bind(ticket.get::<uuid::Uuid, _>("id"))
    .execute(&mut *transaction)
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = sqlx::query(
        "UPDATE candidates SET status = $1::registration_status WHERE uuid = $2 OR uuid = $3",
    )
    .bind(&form.status)
    .bind(ticket.get::<uuid::Uuid, _>("president_id"))
    .bind(new_vp_id)
    .execute(&mut *transaction)
    .await
    {
        return database_candidate_error(error);
    }
    if new_vp_id != ticket.get::<uuid::Uuid, _>("vice_president_id") {
        if let Err(error) =
            sqlx::query("UPDATE candidates SET status = 'invalidated' WHERE uuid = $1")
                .bind(ticket.get::<uuid::Uuid, _>("vice_president_id"))
                .execute(&mut *transaction)
                .await
        {
            return transaction_error(error);
        }
    }
    if let Err(error) = replace_messages(&mut transaction, ticket.get("id"), &messages).await {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "presidential ticket",
        ticket_uuid,
        &previous,
        &new_value,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, %ticket_uuid, actor_citizen_id = actor.id, new_status = %form.status, vice_president_changed = form.vice_president_citizen_id != old_vp_citizen_id, message_count = messages.len(), "Updated presidential ticket");
    Redirect::to(&format!("/manage/elections/{election_uuid}/candidates")).into_response()
}

pub async fn get_election_changes(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Query(search): Query<ChangeSearch>,
) -> Response {
    trace!(%election_uuid, "Handling election change log request");
    let election_name =
        match sqlx::query_scalar::<_, String>("SELECT name FROM elections WHERE uuid = $1")
            .bind(election_uuid)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(name)) => name,
            Ok(None) => {
                debug!(%election_uuid, "Election change log target was not found");
                return not_found();
            }
            Err(error) => return transaction_error(error),
        };
    let timezone = crate::render::timezone(&jar);
    let changes_query = "SELECT actor_display_name, target_type, previous_value, new_value, reason,
                to_char(election_change_log.database_created_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI:SS') || ' Europe/Paris' AS changed_at
         FROM election_change_log
         JOIN elections ON elections.uuid = election_change_log.election_id
         WHERE elections.uuid = $1
         AND ($2 = '%%'
              OR actor_display_name ILIKE $2
              OR target_type ILIKE $2
              OR previous_value ILIKE $2
              OR new_value ILIKE $2
              OR reason ILIKE $2)
         ORDER BY election_change_log.database_created_at DESC, election_change_log.id DESC".replace("Europe/Paris", &timezone);
    let changes = match sqlx::query(sqlx::AssertSqlSafe(changes_query.as_str()))
        .bind(election_uuid)
        .bind(format!("%{}%", search.q.trim()))
        .fetch_all(&state.pool)
        .await
    {
        Ok(changes) => {
            debug!(%election_uuid, change_count = changes.len(), "Retrieved election change log");
            changes
        }
        Err(error) => return transaction_error(error),
    };
    let mut items = Vec::new();
    for change in changes {
        items.push(ElectionChange {
            changed_at: change.get("changed_at"),
            actor: change.get("actor_display_name"),
            target: change.get("target_type"),
            previous: change.get("previous_value"),
            new: change.get("new_value"),
            reason: change.get("reason"),
        });
    }
    let page = ElectionChangesPage {
        election_name: &election_name,
        election_uuid,
        search_query: search.q.trim(),
        changes: &items,
    };
    trace!(%election_uuid, "Rendering election change log page");
    render_template_page(&page, "Election changes", jar, &state.pool)
        .await
        .into_response()
}

async fn eligible_vp_options(
    state: &AppState,
    election_id: uuid::Uuid,
    current_vp: uuid::Uuid,
) -> Result<Vec<ManagedVicePresidentOption>, Response> {
    trace!(
        %election_id,
        %current_vp, "Retrieving manager vice president options"
    );
    let citizens = sqlx::query(
        "SELECT citizens.uuid AS id,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.uuid::text) AS display_name
         FROM citizens
         JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
         WHERE citizens.banned = FALSE
         AND (citizens.uuid = $2 OR NOT EXISTS (
             SELECT 1 FROM candidates
             WHERE candidates.election_id = $1 AND candidates.citizen_id = citizens.uuid
             AND candidates.status = 'active'
         ))
         ORDER BY display_name",
    )
    .bind(election_id)
    .bind(current_vp)
    .fetch_all(&state.pool)
    .await;
    let citizens = match citizens {
        Ok(citizens) => {
            debug!(
                %election_id,
                citizen_count = citizens.len(),
                "Retrieved manager vice president options"
            );
            citizens
        }
        Err(error) => return Err(transaction_error(error)),
    };
    Ok(citizens
        .iter()
        .map(|citizen| {
            let id: uuid::Uuid = citizen.get("id");
            ManagedVicePresidentOption {
                id,
                selected: id == current_vp,
                label: citizen.get("display_name"),
            }
        })
        .collect())
}

async fn ticket_messages(
    state: &AppState,
    ticket_uuid: uuid::Uuid,
) -> Result<Vec<String>, Response> {
    trace!(%ticket_uuid, "Loading managed presidential ticket messages");
    let rows = sqlx::query(
        "SELECT presidential_ticket_messages.position, presidential_ticket_messages.message
         FROM presidential_ticket_messages
         JOIN presidential_tickets ON presidential_tickets.uuid = presidential_ticket_messages.presidential_ticket_id
         WHERE presidential_tickets.uuid = $1 ORDER BY presidential_ticket_messages.position",
    )
    .bind(ticket_uuid)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return Err(transaction_error(error)),
    };
    let mut messages = vec![String::new(); 5];
    for row in rows {
        let position: i32 = row.get("position");
        messages[(position - 1) as usize] = row.get("message");
    }
    debug!(%ticket_uuid, message_count = messages.iter().filter(|message| !message.is_empty()).count(), "Loaded managed presidential ticket messages");
    Ok(messages)
}

fn valid_election_status(status: &str) -> bool {
    let valid = matches!(
        status,
        "upcoming" | "paused" | "closed" | "canceled" | "certified"
    );
    trace!(status, valid, "Validated election status");
    valid
}

fn valid_registration_status(status: &str) -> bool {
    let valid = matches!(status, "active" | "withdrawn" | "invalidated");
    trace!(status, valid, "Validated registration status");
    valid
}

fn validate_candidate_fields<'a>(
    display_name: &str,
    party: &str,
    status: &str,
    reason: &str,
) -> Result<(), &'a str> {
    if display_name.trim().is_empty() || party.trim().is_empty() {
        return Err("Display name and party are required.");
    }
    if !valid_registration_status(status) {
        return Err("Invalid registration status.");
    }
    if reason.trim().is_empty() {
        return Err("A reason is required.");
    }
    Ok(())
}

fn validate_presidential_form(form: &PresidentialTicketForm) -> Result<(), &'static str> {
    validate_candidate_fields(
        &form.president_display_name,
        &form.president_party,
        &form.status,
        &form.reason,
    )?;
    if form.vice_president_display_name.trim().is_empty()
        || form.vice_president_party.trim().is_empty()
    {
        return Err("Vice president display name and party are required.");
    }
    validate_messages(&form_messages(form))
}

fn form_messages(form: &PresidentialTicketForm) -> Vec<String> {
    [
        &form.message_1,
        &form.message_2,
        &form.message_3,
        &form.message_4,
        &form.message_5,
    ]
    .iter()
    .map(|message| message.trim())
    .filter(|message| !message.is_empty())
    .map(str::to_string)
    .collect()
}

fn validate_messages(messages: &[String]) -> Result<(), &'static str> {
    if messages.len() > 5 || messages.iter().any(|message| message.chars().count() > 100) {
        return Err("Tickets may have up to five messages of 100 characters each.");
    }
    Ok(())
}

async fn replace_messages(
    transaction: &mut Transaction<'_, Postgres>,
    ticket_id: uuid::Uuid,
    messages: &[String],
) -> Result<(), sqlx::Error> {
    trace!(
        %ticket_id,
        message_count = messages.len(),
        "Replacing managed ticket messages"
    );
    sqlx::query("DELETE FROM presidential_ticket_messages WHERE presidential_ticket_id = $1")
        .bind(ticket_id)
        .execute(&mut **transaction)
        .await?;
    for (index, message) in messages.iter().enumerate() {
        sqlx::query(
            "INSERT INTO presidential_ticket_messages (presidential_ticket_id, position, message) VALUES ($1, $2, $3)",
        )
        .bind(ticket_id)
        .bind((index + 1) as i32)
        .bind(message)
        .execute(&mut **transaction)
        .await?;
    }
    debug!(
        %ticket_id,
        message_count = messages.len(),
        "Replaced managed ticket messages"
    );
    Ok(())
}

async fn insert_change(
    transaction: &mut Transaction<'_, Postgres>,
    election_uuid: uuid::Uuid,
    actor: &AuthenticatedCitizen,
    target_type: &str,
    target_uuid: uuid::Uuid,
    previous_value: &str,
    new_value: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    trace!(%election_uuid, actor_citizen_id = actor.id, target_type, %target_uuid, "Recording election management change");
    sqlx::query(
        "INSERT INTO election_change_log (
            election_id, actor_citizen_id, actor_display_name, target_type, target_uuid,
            previous_value, new_value, reason
         ) SELECT uuid, $1, $2, $3, $4, $5, $6, $7 FROM elections WHERE uuid = $8",
    )
    .bind(actor.uuid)
    .bind(&actor.display_name)
    .bind(target_type)
    .bind(target_uuid)
    .bind(previous_value)
    .bind(new_value)
    .bind(reason.trim())
    .bind(election_uuid)
    .execute(&mut **transaction)
    .await?;
    debug!(%election_uuid, actor_citizen_id = actor.id, target_type, %target_uuid, "Recorded election management change");
    Ok(())
}

fn bad_request(message: &str) -> Response {
    debug!(
        validation_error = message,
        "Returning invalid election status response"
    );
    error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new(
            "Invalid election change",
            message,
            "invalid-election-change-page",
        )
        .with_back("/manage/elections", "Back to elections"),
    )
}

fn not_found() -> Response {
    debug!("Returning election status not found response");
    error_response(
        StatusCode::NOT_FOUND,
        &ErrorPage::new(
            "Election or candidate not found",
            "",
            "election-status-not-found-page",
        ),
    )
}

fn server_error() -> Response {
    error!("Returning election status server error response");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new(
            "Could not manage election status",
            "",
            "election-status-error-page",
        ),
    )
}

fn transaction_error(error: sqlx::Error) -> Response {
    error!(?error, "Failed to change election status data");
    server_error()
}

fn database_candidate_error(error: sqlx::Error) -> Response {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() {
            warn!("Election status update violated candidate uniqueness");
            return bad_request("That user already has a role in this election.");
        }
    }
    transaction_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_validated() {
        assert!(valid_election_status("paused"));
        assert!(valid_election_status("canceled"));
        assert!(!valid_election_status("unknown"));
    }

    #[test]
    fn messages_are_limited() {
        assert!(validate_messages(&vec!["a".repeat(100); 5]).is_ok());
        assert!(validate_messages(&vec!["a".repeat(101)]).is_err());
        assert!(validate_messages(&vec!["a".to_string(); 6]).is_err());
    }
}
