use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, info, trace, warn};

use crate::pages::auth::{html_escape, require_election_manager};
use crate::pages::login::AppState;
use crate::render::render_page;

#[derive(Deserialize)]
pub struct ElectionForm {
    season: i32,
    name: String,
    registration_starts_at: String,
    registration_ends_at: String,
    voter_code_registration_starts_at: String,
    voter_code_registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    maximum_council_choices: i32,
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

    let mut previous_elections = String::new();
    for election in elections {
        let uuid: uuid::Uuid = election.get("uuid");
        let season: i32 = election.get("season");
        let name: String = election.get("name");
        let status: String = election.get("status");
        trace!(election_uuid = %uuid, season, %status, "Rendering managed election");
        previous_elections.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-elections/election-item.html"
            ))
            .replace("$${{season}}", &season.to_string())
            .replace("$${{name}}", &html_escape(&name))
            .replace("$${{status}}", &html_escape(&status))
            .replace("$${{election_uuid}}", &uuid.to_string()),
        );
    }
    let empty_hidden = if previous_elections.is_empty() {
        ""
    } else {
        "hidden"
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-elections/manage-elections.html"
    ))
    .replace("$${{search_query}}", &html_escape(search.q.trim()))
    .replace("$${{election_items}}", &previous_elections)
    .replace("$${{empty_hidden}}", empty_hidden);
    trace!("Rendering election management page");
    render_page(&content, "Manage elections", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_new_election(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling new election page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-elections/new-election.html"
    ))
    .replace("$${{election_fields}}", &election_fields(None));
    render_page(&content, "Create election", jar, &state.pool)
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
    let election = match sqlx::query(
        "SELECT season, name, status::text AS status,
                COALESCE(to_char(registration_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS registration_starts_at,
                COALESCE(to_char(registration_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS registration_ends_at,
                COALESCE(to_char(voter_code_registration_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voter_code_registration_starts_at,
                COALESCE(to_char(voter_code_registration_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voter_code_registration_ends_at,
                COALESCE(to_char(voting_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voting_starts_at,
                COALESCE(to_char(voting_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voting_ends_at,
                maximum_council_choices
         FROM elections WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(election)) => election,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/manage-elections/not-found.html"
                ))),
            )
                .into_response();
        }
        Err(error) => {
            error!(?error, %election_uuid, "Failed to retrieve election management dashboard");
            return server_error();
        }
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-elections/election-dashboard.html"
    ))
    .replace("$${{election_uuid}}", &election_uuid.to_string())
    .replace(
        "$${{season}}",
        &election.get::<i32, _>("season").to_string(),
    )
    .replace("$${{name}}", &html_escape(election.get("name")))
    .replace("$${{status}}", &html_escape(election.get("status")))
    .replace(
        "$${{registration_starts_at}}",
        &html_escape(election.get("registration_starts_at")),
    )
    .replace(
        "$${{registration_ends_at}}",
        &html_escape(election.get("registration_ends_at")),
    )
    .replace(
        "$${{voter_code_registration_starts_at}}",
        &html_escape(election.get("voter_code_registration_starts_at")),
    )
    .replace(
        "$${{voter_code_registration_ends_at}}",
        &html_escape(election.get("voter_code_registration_ends_at")),
    )
    .replace(
        "$${{voting_starts_at}}",
        &html_escape(election.get("voting_starts_at")),
    )
    .replace(
        "$${{voting_ends_at}}",
        &html_escape(election.get("voting_ends_at")),
    )
    .replace(
        "$${{maximum_council_choices}}",
        &election
            .get::<i32, _>("maximum_council_choices")
            .to_string(),
    );
    debug!(%election_uuid, "Rendering election management dashboard");
    render_page(&content, "Manage election", jar, &state.pool)
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

    let query = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO elections (
            season, name,
            registration_starts_at, registration_ends_at,
            voter_code_registration_starts_at, voter_code_registration_ends_at,
            voting_starts_at, voting_ends_at, maximum_council_choices
        ) VALUES (
            $1, $2,
            NULLIF($3, '')::timestamptz, NULLIF($4, '')::timestamptz,
            NULLIF($5, '')::timestamptz, NULLIF($6, '')::timestamptz,
            NULLIF($7, '')::timestamptz, NULLIF($8, '')::timestamptz, $9
        ) RETURNING uuid",
    )
    .bind(form.season)
    .bind(form.name.trim())
    .bind(&form.registration_starts_at)
    .bind(&form.registration_ends_at)
    .bind(&form.voter_code_registration_starts_at)
    .bind(&form.voter_code_registration_ends_at)
    .bind(&form.voting_starts_at)
    .bind(&form.voting_ends_at)
    .bind(form.maximum_council_choices)
    .fetch_one(&state.pool)
    .await;

    match query {
        Ok(election_uuid) => {
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

    let election = sqlx::query(
        "SELECT season, name,
                to_char(registration_starts_at, 'YYYY-MM-DD\"T\"HH24:MI') AS registration_starts_at,
                to_char(registration_ends_at, 'YYYY-MM-DD\"T\"HH24:MI') AS registration_ends_at,
                to_char(voter_code_registration_starts_at, 'YYYY-MM-DD\"T\"HH24:MI') AS voter_code_registration_starts_at,
                to_char(voter_code_registration_ends_at, 'YYYY-MM-DD\"T\"HH24:MI') AS voter_code_registration_ends_at,
                to_char(voting_starts_at, 'YYYY-MM-DD\"T\"HH24:MI') AS voting_starts_at,
                to_char(voting_ends_at, 'YYYY-MM-DD\"T\"HH24:MI') AS voting_ends_at,
                maximum_council_choices
         FROM elections
         WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await;

    let election = match election {
        Ok(Some(election)) => {
            debug!(%election_uuid, "Retrieved election for editing");
            ElectionForm {
                season: election.get("season"),
                name: election.get("name"),
                registration_starts_at: election
                    .get::<Option<String>, _>("registration_starts_at")
                    .unwrap_or_default(),
                voter_code_registration_starts_at: election
                    .get::<Option<String>, _>("voter_code_registration_starts_at")
                    .unwrap_or_default(),
                voter_code_registration_ends_at: election
                    .get::<Option<String>, _>("voter_code_registration_ends_at")
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
            }
        }
        Ok(None) => {
            debug!(%election_uuid, "Election edit target was not found");
            return (
                StatusCode::NOT_FOUND,
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/manage-elections/not-found.html"
                ))),
            )
                .into_response();
        }
        Err(error) => {
            error!(?error, "Failed to retrieve election");
            return server_error();
        }
    };

    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-elections/edit-election.html"
    ))
    .replace("$${{season}}", &election.season.to_string())
    .replace("$${{election_uuid}}", &election_uuid.to_string())
    .replace("$${{election_fields}}", &election_fields(Some(&election)));
    trace!(%election_uuid, "Rendering election edit page");
    render_page(&content, "Edit election", jar, &state.pool)
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

    let query = sqlx::query(
        "UPDATE elections SET
            season = $1, name = $2,
            registration_starts_at = NULLIF($3, '')::timestamptz,
            registration_ends_at = NULLIF($4, '')::timestamptz,
            voter_code_registration_starts_at = NULLIF($5, '')::timestamptz,
            voter_code_registration_ends_at = NULLIF($6, '')::timestamptz,
            voting_starts_at = NULLIF($7, '')::timestamptz,
            voting_ends_at = NULLIF($8, '')::timestamptz,
            maximum_council_choices = $9
         WHERE uuid = $10",
    )
    .bind(form.season)
    .bind(form.name.trim())
    .bind(&form.registration_starts_at)
    .bind(&form.registration_ends_at)
    .bind(&form.voter_code_registration_starts_at)
    .bind(&form.voter_code_registration_ends_at)
    .bind(&form.voting_starts_at)
    .bind(&form.voting_ends_at)
    .bind(form.maximum_council_choices)
    .bind(election_uuid)
    .execute(&state.pool)
    .await;

    match query {
        Ok(result) if result.rows_affected() == 1 => {
            info!(%election_uuid, season = form.season, "Updated election");
            Redirect::to(&format!("/manage/elections/{election_uuid}")).into_response()
        }
        Ok(result) => {
            debug!(%election_uuid, rows_affected = result.rows_affected(), "Election update target was not found");
            (
                StatusCode::NOT_FOUND,
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/manage-elections/not-found.html"
                ))),
            )
                .into_response()
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
    if form.maximum_council_choices <= 0 {
        return Err("Maximum council choices must be greater than zero.");
    }
    let registration_starts_at = parse_date(&form.registration_starts_at)?;
    let registration_ends_at = parse_date(&form.registration_ends_at)?;
    let voter_code_registration_starts_at = parse_date(&form.voter_code_registration_starts_at)?;
    let voter_code_registration_ends_at = parse_date(&form.voter_code_registration_ends_at)?;
    let voting_starts_at = parse_date(&form.voting_starts_at)?;
    let voting_ends_at = parse_date(&form.voting_ends_at)?;

    if matches!((registration_starts_at, registration_ends_at), (Some(start), Some(end)) if start >= end)
    {
        return Err("Registration must end after it starts.");
    }
    if matches!((voter_code_registration_starts_at, voter_code_registration_ends_at), (Some(start), Some(end)) if start >= end)
    {
        return Err("Voter code registration must end after it starts.");
    }
    if matches!((voting_starts_at, voting_ends_at), (Some(start), Some(end)) if start >= end) {
        return Err("Voting must end after it starts.");
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

fn election_fields(election: Option<&ElectionForm>) -> String {
    trace!(editing = election.is_some(), "Rendering election fields");
    let empty = ElectionForm {
        season: 1,
        name: String::new(),
        registration_starts_at: String::new(),
        registration_ends_at: String::new(),
        voter_code_registration_starts_at: String::new(),
        voter_code_registration_ends_at: String::new(),
        voting_starts_at: String::new(),
        voting_ends_at: String::new(),
        maximum_council_choices: 10,
    };
    let election = election.unwrap_or(&empty);

    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-elections/election-fields.html"
    ))
    .replace("$${{season}}", &election.season.to_string())
    .replace("$${{name}}", &html_escape(&election.name))
    .replace(
        "$${{registration_starts_at}}",
        &html_escape(&election.registration_starts_at),
    )
    .replace(
        "$${{registration_ends_at}}",
        &html_escape(&election.registration_ends_at),
    )
    .replace(
        "$${{voter_code_registration_starts_at}}",
        &html_escape(&election.voter_code_registration_starts_at),
    )
    .replace(
        "$${{voter_code_registration_ends_at}}",
        &html_escape(&election.voter_code_registration_ends_at),
    )
    .replace(
        "$${{voting_starts_at}}",
        &html_escape(&election.voting_starts_at),
    )
    .replace(
        "$${{voting_ends_at}}",
        &html_escape(&election.voting_ends_at),
    )
    .replace(
        "$${{maximum_council_choices}}",
        &election.maximum_council_choices.to_string(),
    )
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

fn bad_request(message: &str) -> Response {
    debug!(
        validation_error = message,
        "Returning invalid election form response"
    );
    (
        StatusCode::BAD_REQUEST,
        Html(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-elections/invalid.html"
            ))
            .replace("$${{message}}", &html_escape(message)),
        ),
    )
        .into_response()
}

fn server_error() -> Response {
    error!("Returning election management server error response");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/manage-elections/error.html"
        ))),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_form() -> ElectionForm {
        ElectionForm {
            season: 1,
            name: "Election".to_string(),
            registration_starts_at: "2026-01-01T09:00".to_string(),
            registration_ends_at: "2026-01-02T09:00".to_string(),
            voter_code_registration_starts_at: "2026-01-03T09:00".to_string(),
            voter_code_registration_ends_at: "2026-01-04T09:00".to_string(),
            voting_starts_at: "2026-01-05T09:00".to_string(),
            voting_ends_at: "2026-01-06T09:00".to_string(),
            maximum_council_choices: 10,
        }
    }

    #[test]
    fn election_periods_are_ordered() {
        assert!(validate_election(&valid_form()).is_ok());

        let mut form = valid_form();
        form.voter_code_registration_ends_at = "2026-01-03T08:00".to_string();
        assert_eq!(
            validate_election(&form),
            Err("Voter code registration must end after it starts.")
        );
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
