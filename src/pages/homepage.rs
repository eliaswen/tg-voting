use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::error;

use crate::pages::auth::{ELECTION_MINISTER, SUPERADMIN, current_citizen, html_escape};
use crate::pages::login::AppState;

pub async fn get_homepage(State(state): State<AppState>, jar: CookieJar) -> Response {
    let account = match current_citizen(&state, &jar).await {
        Ok(Some(citizen)) if citizen.banned => "<p>Your account is banned.</p>".to_string(),
        Ok(Some(citizen)) => {
            let manage = if citizen.role & (ELECTION_MINISTER | SUPERADMIN) != 0 {
                "<p><a href=\"/manage/elections\">Manage elections</a></p>"
            } else {
                ""
            };
            format!(
                "<p>Logged in as {}</p><p><a href=\"/account\">Account</a></p>{}",
                html_escape(&citizen.display_name),
                manage,
            )
        }
        Ok(None) => "<p><a href=\"/login\">Login</a></p>".to_string(),
        Err(response) => return response,
    };

    let elections = match sqlx::query(
        "SELECT uuid, season, name, status::text AS status FROM elections ORDER BY season DESC",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(elections) => elections,
        Err(error) => {
            error!(?error, "Failed to retrieve elections for homepage");
            return Html("<h1>Elections</h1><p>Could not retrieve elections.</p>").into_response();
        }
    };
    let mut items = String::new();
    for election in elections {
        let uuid: uuid::Uuid = election.get("uuid");
        items.push_str(&format!(
            "<li>Season {}: {} ({}) - <a href=\"/elections/{}/candidates\">Candidates</a> - <a href=\"/elections/{}/register\">Register</a> - <a href=\"/elections/{}/changes\">Changes</a></li>",
            election.get::<i32, _>("season"),
            html_escape(election.get("name")),
            html_escape(election.get("status")),
            uuid,
            uuid,
            uuid,
        ));
    }
    if items.is_empty() {
        items.push_str("<li>No elections have been created yet.</li>");
    }
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Elections</title></head><body><h1>Elections</h1>{}<h2>Elections</h2><ul>{}</ul></body></html>",
        account,
        items,
    ))
    .into_response()
}
