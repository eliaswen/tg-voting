use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Deserialize, Default)]
pub struct ElectionSearch {
    #[serde(default)]
    q: String,
}

pub async fn get_elections(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(search): Query<ElectionSearch>,
) -> Response {
    trace!("Handling public elections request");
    trace!("Retrieving visible elections");
    let timezone = crate::render::timezone(&jar);
    let elections_query = "SELECT uuid, season, name, description, election_type::text AS election_type, status::text AS status, paused_stage,
                registration_starts_at AS registration_start_raw, registration_ends_at AS registration_end_raw, voting_starts_at AS voting_start_raw, voting_ends_at AS voting_end_raw,
                to_char(registration_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS registration_starts_at,
                to_char(registration_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS registration_ends_at,
                to_char(voting_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS voting_starts_at,
                to_char(voting_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS voting_ends_at
         FROM elections
         WHERE status <> 'draft'
         AND ($1 = '%%' OR name ILIKE $1 OR season::text ILIKE $1 OR status::text ILIKE $1)
         ORDER BY season DESC".replace("Europe/Paris", &timezone);
    let elections = match sqlx::query(sqlx::AssertSqlSafe(elections_query.as_str()))
        .bind(format!("%{}%", search.q.trim()))
        .fetch_all(&state.pool)
        .await
    {
        Ok(elections) => elections,
        Err(error) => {
            error!(?error, "Failed to retrieve elections");
            return themed_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new(
                    "Elections",
                    "Could not retrieve elections.",
                    "elections-error-page",
                ),
                &state,
                jar,
            )
            .await;
        }
    };
    debug!(
        election_count = elections.len(),
        "Retrieved visible elections"
    );

    let mut items = Vec::new();
    for election in elections {
        let uuid: uuid::Uuid = election.get("uuid");
        let status: String = election.get("status");
        let timeline = crate::pages::election_lifecycle::timeline(
            &status,
            election.get("registration_start_raw"),
            election.get("registration_end_raw"),
            election.get("voting_start_raw"),
            election.get("voting_end_raw"),
            election.get::<Option<String>, _>("paused_stage").as_deref(),
            chrono::Utc::now(),
        );
        trace!(election_uuid = %uuid, %status, "Rendering election summary");
        let registration = date_range(
            election.get("registration_starts_at"),
            election.get("registration_ends_at"),
        );
        let voting = date_range(
            election.get("voting_starts_at"),
            election.get("voting_ends_at"),
        );
        items.push(ElectionListItem {
            uuid,
            season: election.get("season"),
            name: election.get("name"),
            description: election.get("description"),
            election_type: election.get("election_type"),
            next: timeline.next,
            registration,
            voting,
            registration_open: timeline.stage == "registration",
            status: timeline.stage_label,
        });
    }
    let page = ElectionsPage {
        elections: &items,
        search_query: search.q.trim(),
    };

    trace!("Rendering public elections page");
    render_template_page(&page, "Elections", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_election(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%election_uuid, "Handling election overview request");
    let timezone = crate::render::timezone(&jar);
    let election_query = "SELECT season, name, description, election_type::text AS election_type, status::text AS status, paused_stage,
                registration_starts_at AS registration_start_raw, registration_ends_at AS registration_end_raw, voting_starts_at AS voting_start_raw, voting_ends_at AS voting_end_raw,
                COALESCE(to_char(registration_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not scheduled') AS registration_starts_at,
                COALESCE(to_char(registration_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not scheduled') AS registration_ends_at,
                COALESCE(to_char(voting_starts_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not scheduled') AS voting_starts_at,
                COALESCE(to_char(voting_ends_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris', 'Not scheduled') AS voting_ends_at
         FROM elections WHERE uuid = $1 AND status <> 'draft'".replace("Europe/Paris", &timezone);
    let election = match sqlx::query(sqlx::AssertSqlSafe(election_query.as_str()))
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(election)) => election,
        Ok(None) => {
            return crate::error_handling::error_not_found(State(state), jar)
                .await
                .into_response();
        }
        Err(error) => {
            error!(?error, %election_uuid, "Failed to retrieve election overview");
            return themed_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new(
                    "Elections",
                    "Could not retrieve elections.",
                    "elections-error-page",
                ),
                &state,
                jar,
            )
            .await;
        }
    };
    let status: String = election.get("status");
    let timeline = crate::pages::election_lifecycle::timeline(
        &status,
        election.get("registration_start_raw"),
        election.get("registration_end_raw"),
        election.get("voting_start_raw"),
        election.get("voting_end_raw"),
        election.get::<Option<String>, _>("paused_stage").as_deref(),
        chrono::Utc::now(),
    );
    let page = ElectionPage {
        election_uuid,
        season: election.get("season"),
        name: election.get("name"),
        description: election.get("description"),
        election_type: election.get("election_type"),
        next: timeline.next,
        registration_starts_at: election.get("registration_starts_at"),
        registration_ends_at: election.get("registration_ends_at"),
        voting_starts_at: election.get("voting_starts_at"),
        voting_ends_at: election.get("voting_ends_at"),
        registration_open: timeline.stage == "registration",
        code_open: matches!(
            timeline.stage.as_str(),
            "registration" | "upcoming" | "voting"
        ) && chrono::Utc::now()
            >= election
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("registration_start_raw")
                .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC),
        voting_open: timeline.stage == "voting",
        status: timeline.stage_label,
    };
    render_template_page(&page, "Election", jar, &state.pool)
        .await
        .into_response()
}

fn date_range(start: Option<String>, end: Option<String>) -> String {
    match (start, end) {
        (Some(start), Some(end)) => format!("{start} to {end}"),
        (Some(start), None) => format!("From {start}"),
        (None, Some(end)) => format!("Until {end}"),
        (None, None) => "Not scheduled".to_string(),
    }
}

struct ElectionListItem {
    uuid: uuid::Uuid,
    season: i32,
    name: String,
    description: String,
    election_type: String,
    next: String,
    status: String,
    registration: String,
    voting: String,
    registration_open: bool,
}

#[derive(Template)]
#[template(path = "elections/elections.html")]
struct ElectionsPage<'a> {
    elections: &'a [ElectionListItem],
    search_query: &'a str,
}

#[derive(Template)]
#[template(path = "elections/election.html")]
struct ElectionPage {
    election_uuid: uuid::Uuid,
    season: i32,
    name: String,
    description: String,
    election_type: String,
    next: String,
    status: String,
    registration_starts_at: String,
    registration_ends_at: String,
    voting_starts_at: String,
    voting_ends_at: String,
    registration_open: bool,
    code_open: bool,
    voting_open: bool,
}
