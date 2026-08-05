use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use tracing::error;

use crate::pages::auth::{html_escape, require_election_manager};
use crate::pages::login::AppState;

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

pub async fn get_manage_elections(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }

    let elections = sqlx::query(
        "SELECT uuid, season, name, status::text AS status
         FROM elections
         ORDER BY season DESC",
    )
    .fetch_all(&state.pool)
    .await;

    let elections = match elections {
        Ok(elections) => elections,
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
        previous_elections.push_str(&format!(
            "<li>Season {}: {} ({}) - <a href=\"/manage/elections/{}/edit\">Edit</a> - <a href=\"/manage/elections/{}/status\">Status</a> - <a href=\"/elections/{}/candidates\">Candidates</a> - <a href=\"/elections/{}/changes\">Changes</a></li>",
            season,
            html_escape(&name),
            html_escape(&status),
            uuid,
            uuid,
            uuid,
            uuid,
        ));
    }
    if previous_elections.is_empty() {
        previous_elections.push_str("<li>No elections have been created yet.</li>");
    }

    Html(format!(
        "<!doctype html>
        <html lang=\"en\">
        <head><meta charset=\"utf-8\"><title>Manage elections</title></head>
        <body>
            <h1>Manage elections</h1>
            <h2>Create a new season</h2>
            <form method=\"post\" action=\"/manage/elections\">
                {}
                <button type=\"submit\">Create season</button>
            </form>
            <h2>Previous seasons</h2>
            <ul>{}</ul>
        </body>
        </html>",
        election_fields(None),
        previous_elections,
    ))
    .into_response()
}

pub async fn post_manage_elections(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ElectionForm>,
) -> Response {
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    if let Err(message) = validate_election(&form) {
        return bad_request(message);
    }

    let query = sqlx::query(
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
        )",
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
    .execute(&state.pool)
    .await;

    match query {
        Ok(_) => Redirect::to("/manage/elections").into_response(),
        Err(error) => database_form_error(error),
    }
}

pub async fn get_edit_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
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
        Ok(Some(election)) => ElectionForm {
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
        },
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Html("<h1>Election not found</h1>")).into_response();
        }
        Err(error) => {
            error!(?error, "Failed to retrieve election");
            return server_error();
        }
    };

    Html(format!(
        "<!doctype html>
        <html lang=\"en\">
        <head><meta charset=\"utf-8\"><title>Edit election</title></head>
        <body>
            <h1>Edit season {}</h1>
            <form method=\"post\" action=\"/manage/elections/{}/edit\">
                {}
                <button type=\"submit\">Save election</button>
            </form>
            <p><a href=\"/manage/elections\">Back to elections</a></p>
        </body>
        </html>",
        election.season,
        election_uuid,
        election_fields(Some(&election)),
    ))
    .into_response()
}

pub async fn post_edit_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<ElectionForm>,
) -> Response {
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    if let Err(message) = validate_election(&form) {
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
            Redirect::to("/manage/elections").into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Html("<h1>Election not found</h1>")).into_response(),
        Err(error) => database_form_error(error),
    }
}

fn validate_election(form: &ElectionForm) -> Result<(), &'static str> {
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
    Ok(())
}

fn parse_date(value: &str) -> Result<Option<NaiveDateTime>, &'static str> {
    if value.is_empty() {
        return Ok(None);
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .map(Some)
        .map_err(|_| "An election date is invalid.")
}

fn election_fields(election: Option<&ElectionForm>) -> String {
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

    format!(
        "<p><label>Season <input type=\"number\" name=\"season\" min=\"1\" value=\"{}\" required></label></p>
        <p><label>Name <input type=\"text\" name=\"name\" value=\"{}\" required></label></p>
        <p><label>Registration starts <input type=\"datetime-local\" name=\"registration_starts_at\" value=\"{}\"></label></p>
        <p><label>Registration ends <input type=\"datetime-local\" name=\"registration_ends_at\" value=\"{}\"></label></p>
        <p><label>Voter code registration starts <input type=\"datetime-local\" name=\"voter_code_registration_starts_at\" value=\"{}\"></label></p>
        <p><label>Voter code registration ends <input type=\"datetime-local\" name=\"voter_code_registration_ends_at\" value=\"{}\"></label></p>
        <p><label>Voting starts <input type=\"datetime-local\" name=\"voting_starts_at\" value=\"{}\"></label></p>
        <p><label>Voting ends <input type=\"datetime-local\" name=\"voting_ends_at\" value=\"{}\"></label></p>
        <p><label>Maximum council choices <input type=\"number\" name=\"maximum_council_choices\" min=\"1\" value=\"{}\" required></label></p>",
        election.season,
        html_escape(&election.name),
        html_escape(&election.registration_starts_at),
        html_escape(&election.registration_ends_at),
        html_escape(&election.voter_code_registration_starts_at),
        html_escape(&election.voter_code_registration_ends_at),
        html_escape(&election.voting_starts_at),
        html_escape(&election.voting_ends_at),
        election.maximum_council_choices,
    )
}

fn database_form_error(error: sqlx::Error) -> Response {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() {
            return bad_request("That season already exists.");
        }
    }
    error!(?error, "Failed to save election");
    server_error()
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html(format!("<h1>Invalid election</h1><p>{}</p><p><a href=\"/manage/elections\">Back to elections</a></p>", html_escape(message))),
    )
        .into_response()
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("<h1>Could not manage elections</h1>"),
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
