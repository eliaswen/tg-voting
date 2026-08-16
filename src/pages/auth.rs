use axum::{
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace, warn};

use crate::backend::login_oauth::hash_token;
use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::login::AppState;

pub const ELECTION_MINISTER: i64 = 1 << 3;
pub const CENSUS_MINISTER: i64 = 1 << 2;
pub const SUPERADMIN: i64 = 1 << 5;

pub struct AuthenticatedCitizen {
    pub id: i64,
    pub uuid: uuid::Uuid,
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
        "SELECT citizens.id, citizens.uuid, citizens.role, citizens.banned,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.id::text) AS display_name
         FROM sessions
         JOIN citizens ON citizens.uuid = sessions.associated_citizen_id
         LEFT JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
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
                uuid: citizen.get("uuid"),
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
            Err(themed_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::permission("401 - Unauthorized", "authorization-error-page"),
                state,
                jar.clone(),
            )
            .await)
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
            Err(themed_error_response(
                StatusCode::FORBIDDEN,
                &ErrorPage::permission("403 Forbidden", "forbidden-page"),
                state,
                jar.clone(),
            )
            .await)
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
        return Err(themed_error_response(
            StatusCode::FORBIDDEN,
            &ErrorPage::permission("403 Forbidden", "forbidden-page"),
            state,
            jar.clone(),
        )
        .await);
    }
    debug!(
        citizen_id = citizen.id,
        role = citizen.role,
        "Election manager authorization succeeded"
    );
    Ok(citizen)
}

pub async fn require_census_manager(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedCitizen, Response> {
    trace!("Requiring census manager");
    let citizen = require_citizen(state, jar).await?;
    if citizen.role & (CENSUS_MINISTER | SUPERADMIN) == 0 {
        warn!(
            citizen_id = citizen.id,
            role = citizen.role,
            "Rejected citizen without census management permission"
        );
        return Err(themed_error_response(
            StatusCode::FORBIDDEN,
            &ErrorPage::permission("403 Forbidden", "forbidden-page"),
            state,
            jar.clone(),
        )
        .await);
    }
    debug!(
        citizen_id = citizen.id,
        role = citizen.role,
        "Census manager authorization succeeded"
    );
    Ok(citizen)
}
