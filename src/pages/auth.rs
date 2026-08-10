use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace, warn};

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
    trace!(
        session_present = jar.get("session").is_some(),
        "Resolving current citizen"
    );
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!("No session cookie was supplied");
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
        Ok(Some(citizen)) => {
            let citizen = AuthenticatedCitizen {
                id: citizen.get("id"),
                role: citizen.get("role"),
                banned: citizen.get("banned"),
                display_name: citizen.get("display_name"),
            };
            debug!(
                citizen_id = citizen.id,
                role = citizen.role,
                banned = citizen.banned,
                "Resolved authenticated citizen"
            );
            Ok(Some(citizen))
        }
        Ok(None) => {
            debug!("Session did not resolve to an active citizen");
            Ok(None)
        }
        Err(error) => {
            error!(?error, "Failed to retrieve authenticated citizen");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/errors/authorization.html"
                ))),
            )
                .into_response())
        }
    }
}

pub async fn require_citizen(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedCitizen, Response> {
    trace!("Requiring authenticated citizen");
    match current_citizen(state, jar).await? {
        Some(citizen) if !citizen.banned => {
            debug!(citizen_id = citizen.id, "Citizen authorization succeeded");
            Ok(citizen)
        }
        Some(citizen) => {
            warn!(citizen_id = citizen.id, "Rejected banned citizen");
            Err((
                StatusCode::FORBIDDEN,
                Html(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/errors/forbidden.html"
                ))),
            )
                .into_response())
        }
        None => {
            debug!("Redirecting unauthenticated request to login");
            Err(Redirect::to("/login").into_response())
        }
    }
}

pub async fn require_election_manager(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedCitizen, Response> {
    trace!("Requiring election manager");
    let citizen = require_citizen(state, jar).await?;
    if citizen.role & (ELECTION_MINISTER | SUPERADMIN) == 0 {
        warn!(
            citizen_id = citizen.id,
            role = citizen.role,
            "Rejected citizen without election management permission"
        );
        return Err((
            StatusCode::FORBIDDEN,
            Html(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/errors/forbidden.html"
            ))),
        )
            .into_response());
    }
    debug!(
        citizen_id = citizen.id,
        role = citizen.role,
        "Election manager authorization succeeded"
    );
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
