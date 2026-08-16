use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, info, trace, warn};

use crate::error_handling::{ErrorPage, error_response};
use crate::pages::auth::require_election_manager;
use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Deserialize)]
pub struct ElectionForm {
    season: i32,
    name: String,
    description: String,
    election_type: String,
    #[serde(default)]
    president: Option<String>,
    #[serde(default)]
    council: Option<String>,
    #[serde(default)]
    ombudsman: Option<String>,
    #[serde(default)]
    moderator: Option<String>,
    #[serde(default)]
    moderator_placeholder_1: Option<String>,
    #[serde(default)]
    moderator_placeholder_2: Option<String>,
    registration_starts_at: String,
    registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    maximum_council_choices: i32,
    #[serde(default)]
    force_edit: bool,
}

#[derive(Deserialize, Default)]
pub struct ManageElectionSearch {
    #[serde(default)]
    q: String,
}

pub async fn get_manage_elections(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(search): Query<ManageElectionSearch>,
) -> Response {
    trace!("Handling election management page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }

    let elections = sqlx::query(
        "SELECT uuid, season, name, status::text AS status
         FROM elections
         WHERE $1 = '%%' OR name ILIKE $1 OR season::text ILIKE $1 OR status::text ILIKE $1
         ORDER BY season DESC",
    )
    .bind(format!("%{}%", search.q.trim()))
    .fetch_all(&state.pool)
    .await;

    let elections = match elections {
        Ok(elections) => {
            debug!(
                election_count = elections.len(),
                "Retrieved elections for management page"
            );
            elections
        }
        Err(error) => {
            error!(?error, "Failed to retrieve elections");
            return server_error();
        }
    };

    let mut previous_elections = Vec::new();
    for election in elections {
        let uuid: uuid::Uuid = election.get("uuid");
        let season: i32 = election.get("season");
        let name: String = election.get("name");
        let status: String = election.get("status");
        trace!(election_uuid = %uuid, season, %status, "Rendering managed election");
        previous_elections.push(ManagedElectionItem {
            uuid,
            season,
            name,
            status,
        });
    }
    let page = ManageElectionsPage {
        search_query: search.q.trim(),
        elections: &previous_elections,
    };
    trace!("Rendering election management page");
    render_template_page(&page, "Manage elections", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_new_election(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling new election page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    render_template_page(
        &NewElectionPage {
            form: ElectionForm::default(),
        },
        "Create election",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn get_manage_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%election_uuid, "Handling election management dashboard request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    let timezone = crate::render::timezone(&jar);
    let query = "SELECT season, name, description, status::text AS status, paused_stage,
                registration_starts_at AS registration_start_raw, registration_ends_at AS registration_end_raw,
                voting_starts_at AS voting_start_raw, voting_ends_at AS voting_end_raw,
                COALESCE(to_char(registration_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS registration_starts_at,
                COALESCE(to_char(registration_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS registration_ends_at,
                election_type::text AS election_type,
                COALESCE(to_char(voting_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS voting_starts_at,
                COALESCE(to_char(voting_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not set') AS voting_ends_at,
                maximum_council_choices
         FROM elections WHERE uuid = $1".replace("Europe/Paris", &timezone);
    let election = match sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(election)) => election,
        Ok(None) => {
            return election_not_found();
        }
        Err(error) => {
            error!(?error, %election_uuid, "Failed to retrieve election management dashboard");
            return server_error();
        }
    };
    let stored_status: String = election.get("status");
    let effective = crate::pages::election_lifecycle::timeline(
        &stored_status,
        election.get("registration_start_raw"),
        election.get("registration_end_raw"),
        election.get("voting_start_raw"),
        election.get("voting_end_raw"),
        election.get::<Option<String>, _>("paused_stage").as_deref(),
        chrono::Utc::now(),
    );
    let page = ElectionDashboardPage {
        election_uuid,
        season: election.get("season"),
        name: election.get("name"),
        description: election.get("description"),
        election_type: election.get("election_type"),
        status: effective.stage_label,
        registration_starts_at: election.get("registration_starts_at"),
        registration_ends_at: election.get("registration_ends_at"),
        voting_starts_at: election.get("voting_starts_at"),
        voting_ends_at: election.get("voting_ends_at"),
        maximum_council_choices: election.get("maximum_council_choices"),
    };
    debug!(%election_uuid, "Rendering election management dashboard");
    render_template_page(&page, "Manage election", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_manage_elections(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ElectionForm>,
) -> Response {
    trace!(season = form.season, "Handling election creation");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    if let Err(message) = validate_election(&form) {
        warn!(
            season = form.season,
            validation_error = message,
            "Rejected invalid election creation"
        );
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return database_form_error(error),
    };
    let timezone = crate::render::timezone(&jar);
    let create_query = "INSERT INTO elections (
            season, name, description, election_type,
            registration_starts_at, registration_ends_at,
            voting_starts_at, voting_ends_at, maximum_council_choices
        ) VALUES (
            $1, $2, $3, $4::election_type,
            NULLIF($5, '')::timestamp AT TIME ZONE 'Europe/Paris', NULLIF($6, '')::timestamp AT TIME ZONE 'Europe/Paris',
            NULLIF($7, '')::timestamp AT TIME ZONE 'Europe/Paris', NULLIF($8, '')::timestamp AT TIME ZONE 'Europe/Paris', $9
        ) RETURNING uuid".replace("Europe/Paris", &timezone);
    let query = sqlx::query_scalar::<_, uuid::Uuid>(sqlx::AssertSqlSafe(create_query.as_str()))
        .bind(form.season)
        .bind(form.name.trim())
        .bind(form.description.trim())
        .bind(&form.election_type)
        .bind(&form.registration_starts_at)
        .bind(&form.registration_ends_at)
        .bind(&form.voting_starts_at)
        .bind(&form.voting_ends_at)
        .bind(form.maximum_council_choices)
        .fetch_one(&mut *transaction)
        .await;

    match query {
        Ok(election_uuid) => {
            if let Err(error) =
                save_positions(&mut transaction, election_uuid, &form.selected_positions()).await
            {
                return database_form_error(error);
            }
            if let Err(error) = transaction.commit().await {
                return database_form_error(error);
            }
            info!(
                season = form.season,
                %election_uuid,
                "Created election"
            );
            Redirect::to(&format!("/manage/elections/{election_uuid}")).into_response()
        }
        Err(error) => database_form_error(error),
    }
}

pub async fn get_edit_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%election_uuid, "Handling election edit page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }

    let timezone = crate::render::timezone(&jar);
    let edit_query = "SELECT season, name, description,
                to_char(registration_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD\"T\"HH24:MI') AS registration_starts_at,
                to_char(registration_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD\"T\"HH24:MI') AS registration_ends_at,
                election_type::text AS election_type, status::text AS status,
                to_char(voting_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD\"T\"HH24:MI') AS voting_starts_at,
                to_char(voting_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD\"T\"HH24:MI') AS voting_ends_at,
                maximum_council_choices
         FROM elections
         WHERE uuid = $1".replace("Europe/Paris", &timezone);
    let election = sqlx::query(sqlx::AssertSqlSafe(edit_query.as_str()))
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await;

    let election = match election {
        Ok(Some(election)) => {
            debug!(%election_uuid, "Retrieved election for editing");
            let positions = sqlx::query_scalar::<_, String>("SELECT position::text FROM election_positions WHERE election_id = $1 AND position <> 'vice_president'").bind(election_uuid).fetch_all(&state.pool).await.unwrap_or_default();
            ElectionForm {
                season: election.get("season"),
                name: election.get("name"),
                description: election.get("description"),
                election_type: election.get("election_type"),
                president: positions
                    .iter()
                    .any(|position| position == "president")
                    .then(|| "president".to_string()),
                council: positions
                    .iter()
                    .any(|position| position == "council")
                    .then(|| "council".to_string()),
                ombudsman: positions
                    .iter()
                    .any(|position| position == "ombudsman")
                    .then(|| "ombudsman".to_string()),
                moderator: positions
                    .iter()
                    .any(|position| position == "moderator")
                    .then(|| "moderator".to_string()),
                moderator_placeholder_1: positions
                    .iter()
                    .any(|position| position == "moderator_placeholder_1")
                    .then(|| "moderator_placeholder_1".to_string()),
                moderator_placeholder_2: positions
                    .iter()
                    .any(|position| position == "moderator_placeholder_2")
                    .then(|| "moderator_placeholder_2".to_string()),
                registration_starts_at: election
                    .get::<Option<String>, _>("registration_starts_at")
                    .unwrap_or_default(),
                registration_ends_at: election
                    .get::<Option<String>, _>("registration_ends_at")
                    .unwrap_or_default(),
                voting_starts_at: election
                    .get::<Option<String>, _>("voting_starts_at")
                    .unwrap_or_default(),
                voting_ends_at: election
                    .get::<Option<String>, _>("voting_ends_at")
                    .unwrap_or_default(),
                maximum_council_choices: election.get("maximum_council_choices"),
                force_edit: false,
            }
        }
        Ok(None) => {
            debug!(%election_uuid, "Election edit target was not found");
            return election_not_found();
        }
        Err(error) => {
            error!(?error, "Failed to retrieve election");
            return server_error();
        }
    };

    let page = EditElectionPage {
        election_uuid,
        form: election,
        debug_mode: state.app_mode == 0,
    };
    trace!(%election_uuid, "Rendering election edit page");
    render_template_page(&page, "Edit election", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_edit_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<ElectionForm>,
) -> Response {
    trace!(%election_uuid, season = form.season, "Handling election update");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    if let Err(message) = validate_election(&form) {
        warn!(%election_uuid, season = form.season, validation_error = message, "Rejected invalid election update");
        return bad_request(message);
    }

    let stored_status =
        sqlx::query_scalar::<_, String>("SELECT status::text FROM elections WHERE uuid = $1")
            .bind(election_uuid)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    let force_edit = state.app_mode == 0 && form.force_edit;
    if stored_status.as_deref() != Some("draft") && !force_edit {
        return bad_request("Only draft elections can be edited.");
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return database_form_error(error),
    };
    let timezone = crate::render::timezone(&jar);
    let update_query = "UPDATE elections SET
            season = $1, name = $2, description = $3, election_type = $4::election_type,
            registration_starts_at = NULLIF($5, '')::timestamp AT TIME ZONE 'Europe/Paris',
            registration_ends_at = NULLIF($6, '')::timestamp AT TIME ZONE 'Europe/Paris',
            voting_starts_at = NULLIF($7, '')::timestamp AT TIME ZONE 'Europe/Paris',
            voting_ends_at = NULLIF($8, '')::timestamp AT TIME ZONE 'Europe/Paris',
            maximum_council_choices = $9
         WHERE uuid = $10"
        .replace("Europe/Paris", &timezone);
    let query = sqlx::query(sqlx::AssertSqlSafe(update_query.as_str()))
        .bind(form.season)
        .bind(form.name.trim())
        .bind(form.description.trim())
        .bind(&form.election_type)
        .bind(&form.registration_starts_at)
        .bind(&form.registration_ends_at)
        .bind(&form.voting_starts_at)
        .bind(&form.voting_ends_at)
        .bind(form.maximum_council_choices)
        .bind(election_uuid)
        .execute(&mut *transaction)
        .await;

    match query {
        Ok(result) if result.rows_affected() == 1 => {
            if let Err(error) = sqlx::query("DELETE FROM election_positions WHERE election_id = $1")
                .bind(election_uuid)
                .execute(&mut *transaction)
                .await
            {
                return database_form_error(error);
            }
            if let Err(error) =
                save_positions(&mut transaction, election_uuid, &form.selected_positions()).await
            {
                return database_form_error(error);
            }
            if let Err(error) = transaction.commit().await {
                return database_form_error(error);
            }
            info!(%election_uuid, season = form.season, "Updated election");
            Redirect::to(&format!("/manage/elections/{election_uuid}")).into_response()
        }
        Ok(result) => {
            debug!(%election_uuid, rows_affected = result.rows_affected(), "Election update target was not found");
            election_not_found()
        }
        Err(error) => database_form_error(error),
    }
}

fn validate_election(form: &ElectionForm) -> Result<(), &'static str> {
    trace!(
        season = form.season,
        maximum_council_choices = form.maximum_council_choices,
        "Validating election form"
    );
    if form.season <= 0 {
        return Err("Season must be greater than zero.");
    }
    if form.name.trim().is_empty() {
        return Err("Name is required.");
    }
    if form.description.trim().is_empty() {
        return Err("Description is required.");
    }
    if form.maximum_council_choices <= 0 {
        return Err("Maximum council choices must be greater than zero.");
    }
    let registration_starts_at = parse_date(&form.registration_starts_at)?;
    let registration_ends_at = parse_date(&form.registration_ends_at)?;
    let voting_starts_at = parse_date(&form.voting_starts_at)?;
    let voting_ends_at = parse_date(&form.voting_ends_at)?;

    if matches!((registration_starts_at, registration_ends_at), (Some(start), Some(end)) if start >= end)
    {
        return Err("Registration must end after it starts.");
    }
    if matches!((voting_starts_at, voting_ends_at), (Some(start), Some(end)) if start >= end) {
        return Err("Voting must end after it starts.");
    }
    if !matches!(form.election_type.as_str(), "general" | "special") {
        return Err("Election type is invalid.");
    }
    if form.selected_positions().is_empty() {
        return Err("Select at least one valid position.");
    }
    if matches!((registration_starts_at, registration_ends_at, voting_starts_at, voting_ends_at), (Some(a), Some(b), Some(c), Some(d)) if !(a < b && b <= c && c < d))
    {
        return Err("Election dates must be in chronological order.");
    }
    trace!(season = form.season, "Election form validation succeeded");
    Ok(())
}

fn parse_date(value: &str) -> Result<Option<NaiveDateTime>, &'static str> {
    if value.is_empty() {
        trace!("Accepted empty election date");
        return Ok(None);
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .map(Some)
        .map_err(|_| "An election date is invalid.")
}

impl Default for ElectionForm {
    fn default() -> Self {
        Self {
            season: 1,
            name: String::new(),
            description: String::new(),
            election_type: "general".to_string(),
            president: None,
            council: None,
            ombudsman: None,
            moderator: None,
            moderator_placeholder_1: None,
            moderator_placeholder_2: None,
            registration_starts_at: String::new(),
            registration_ends_at: String::new(),
            voting_starts_at: String::new(),
            voting_ends_at: String::new(),
            maximum_council_choices: 10,
            force_edit: false,
        }
    }
}

impl ElectionForm {
    fn selected_positions(&self) -> Vec<String> {
        [
            ("president", self.president.is_some()),
            ("council", self.council.is_some()),
            ("ombudsman", self.ombudsman.is_some()),
            ("moderator", self.moderator.is_some()),
            (
                "moderator_placeholder_1",
                self.moderator_placeholder_1.is_some(),
            ),
            (
                "moderator_placeholder_2",
                self.moderator_placeholder_2.is_some(),
            ),
        ]
        .into_iter()
        .filter(|(_, selected)| *selected)
        .map(|(position, _)| position.to_string())
        .collect()
    }
}

fn database_form_error(error: sqlx::Error) -> Response {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() {
            warn!("Election save violated season uniqueness");
            return bad_request("That season already exists.");
        }
    }
    error!(?error, "Failed to save election");
    server_error()
}

struct ManagedElectionItem {
    uuid: uuid::Uuid,
    season: i32,
    name: String,
    status: String,
}

#[derive(Template)]
#[template(path = "manage-elections/manage-elections.html")]
struct ManageElectionsPage<'a> {
    search_query: &'a str,
    elections: &'a [ManagedElectionItem],
}

#[derive(Template)]
#[template(path = "manage-elections/new-election.html")]
struct NewElectionPage {
    form: ElectionForm,
}

#[derive(Template)]
#[template(path = "manage-elections/edit-election.html")]
struct EditElectionPage {
    election_uuid: uuid::Uuid,
    form: ElectionForm,
    debug_mode: bool,
}

#[derive(Template)]
#[template(path = "manage-elections/election-dashboard.html")]
struct ElectionDashboardPage {
    election_uuid: uuid::Uuid,
    season: i32,
    name: String,
    description: String,
    election_type: String,
    status: String,
    registration_starts_at: String,
    registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    maximum_council_choices: i32,
}

async fn save_positions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    election_uuid: uuid::Uuid,
    positions: &[String],
) -> Result<(), sqlx::Error> {
    for position in positions {
        sqlx::query("INSERT INTO election_positions (election_id, position) VALUES ($1, $2::candidate_position) ON CONFLICT DO NOTHING").bind(election_uuid).bind(position).execute(&mut **transaction).await?;
        if position == "president" {
            sqlx::query("INSERT INTO election_positions (election_id, position) VALUES ($1, 'vice_president') ON CONFLICT DO NOTHING").bind(election_uuid).execute(&mut **transaction).await?;
        }
    }
    Ok(())
}

fn bad_request(message: &str) -> Response {
    debug!(
        validation_error = message,
        "Returning invalid election form response"
    );
    error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new("Invalid election", message, "invalid-election-page")
            .with_back("/manage/elections", "Back to elections"),
    )
}

fn election_not_found() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &ErrorPage::new("Election not found", "", "election-not-found-page"),
    )
}

fn server_error() -> Response {
    error!("Returning election management server error response");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new(
            "Could not manage elections",
            "",
            "manage-elections-error-page",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_form() -> ElectionForm {
        ElectionForm {
            season: 1,
            name: "Election".to_string(),
            description: "Election description".to_string(),
            election_type: "general".to_string(),
            president: None,
            council: Some("council".to_string()),
            ombudsman: None,
            moderator: None,
            moderator_placeholder_1: None,
            moderator_placeholder_2: None,
            registration_starts_at: "2026-01-01T09:00".to_string(),
            registration_ends_at: "2026-01-02T09:00".to_string(),
            voting_starts_at: "2026-01-05T09:00".to_string(),
            voting_ends_at: "2026-01-06T09:00".to_string(),
            maximum_council_choices: 10,
            force_edit: false,
        }
    }

    #[test]
    fn election_periods_are_ordered() {
        assert!(validate_election(&valid_form()).is_ok());

        let mut form = valid_form();
        form.voting_starts_at = "2026-01-01T08:00".to_string();
        assert!(validate_election(&form).is_err());
    }

    #[test]
    fn election_fields_are_required() {
        let mut form = valid_form();
        form.name = " ".to_string();
        assert!(validate_election(&form).is_err());

        let mut form = valid_form();
        form.maximum_council_choices = 0;
        assert!(validate_election(&form).is_err());
    }
}
