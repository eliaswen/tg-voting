use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::error;

use crate::backend::login_oauth::hash_token;
use crate::pages::login::AppState;

pub const ELECTION_MINISTER: i64 = 1 << 3;
pub const SUPERADMIN: i64 = 1 << 5;

pub struct AuthenticatedCitizen {
    pub id: i64,
    pub role: i64,
    pub banned: bool,
    pub display_name: String,
}

pub async fn current_citizen(
    state: &AppState,
    jar: &CookieJar,
) -> Result<Option<AuthenticatedCitizen>, Response> {
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        return Ok(None);
    };

    let citizen = sqlx::query(
        "SELECT citizens.id, citizens.role, citizens.banned,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.id::text) AS display_name
         FROM sessions
         JOIN citizens ON citizens.id = sessions.associated_citizen_id
         LEFT JOIN authentik_identities ON authentik_identities.citizen_id = citizens.id
         WHERE sessions.auth_code_hash = $1
         AND sessions.expires_at > CURRENT_TIMESTAMP
         AND sessions.revoked_at IS NULL",
    )
    .bind(hash_token(&session_token))
    .fetch_optional(&state.pool)
    .await;

    match citizen {
        Ok(Some(citizen)) => Ok(Some(AuthenticatedCitizen {
            id: citizen.get("id"),
            role: citizen.get("role"),
            banned: citizen.get("banned"),
            display_name: citizen.get("display_name"),
        })),
        Ok(None) => Ok(None),
        Err(error) => {
            error!(?error, "Failed to retrieve authenticated citizen");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<h1>Could not authorize this request</h1>"),
            )
                .into_response())
        }
    }
}

pub async fn require_citizen(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedCitizen, Response> {
    match current_citizen(state, jar).await? {
        Some(citizen) if !citizen.banned => Ok(citizen),
        Some(_) => Err((StatusCode::FORBIDDEN, Html("<h1>403 Forbidden</h1>")).into_response()),
        None => Err(Redirect::to("/login").into_response()),
    }
}

pub async fn require_election_manager(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedCitizen, Response> {
    let citizen = require_citizen(state, jar).await?;
    if citizen.role & (ELECTION_MINISTER | SUPERADMIN) == 0 {
        return Err((StatusCode::FORBIDDEN, Html("<h1>403 Forbidden</h1>")).into_response());
    }
    Ok(citizen)
}

pub fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
