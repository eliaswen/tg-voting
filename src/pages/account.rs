use crate::pages::auth::html_escape;
use crate::pages::login::{AppState, logout_headers};
use crate::pages::settings::theme_cookie;
use crate::render::{render_page, theme_name, theme_options};
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, info, trace, warn};

#[derive(Deserialize)]
pub struct AccountThemeForm {
    theme: u8,
}

pub async fn get_account_page(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling account page request");
    let session_token = match jar.get("session") {
        Some(cookie) => cookie.value().to_string(),
        None => {
            debug!("Redirecting account page request without a session");
            return Redirect::to("/login").into_response();
        }
    };

    let auth_code_hash = crate::backend::login_oauth::hash_token(&session_token);

    trace!("Retrieving account details");
    let query = sqlx::query(
        "SELECT citizens.id AS citizen_id, authentik_identities.preferred_username, authentik_identities.email, authentik_identities.display_name,
                COALESCE(user_setting.setting_value, '0') AS theme
        FROM sessions
        JOIN authentik_identities
        ON authentik_identities.citizen_id = sessions.associated_citizen_id
        JOIN citizens ON citizens.id = sessions.associated_citizen_id
        LEFT JOIN user_setting ON user_setting.user_uuid = citizens.uuid AND user_setting.setting_key = 'theme'
        WHERE sessions.auth_code_hash = $1
        AND sessions.expires_at > CURRENT_TIMESTAMP
        AND sessions.revoked_at IS NULL"
    )
    .bind(&auth_code_hash)
    .fetch_optional(&state.pool)
    .await;

    match query {
        Ok(Some(row)) => {
            let citizen_id: i64 = row.get("citizen_id");
            debug!(citizen_id, "Retrieved account details");
            let username: String = row.try_get("preferred_username").unwrap_or_default();
            let email: String = row.try_get("email").unwrap_or_default();
            let display_name: String = row.try_get("display_name").unwrap_or_default();
            let theme = row
                .try_get::<String, _>("theme")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .unwrap_or(0);
            trace!(citizen_id, "Retrieving active account sessions");
            let sessions = sqlx::query(
                "SELECT uuid, COALESCE(device_type, 'Unknown') AS device_type,
                        COALESCE(device_name, 'Unknown device') AS device_name,
                        created_at, expires_at, auth_code_hash = $2 AS current_session
                 FROM sessions
                 WHERE associated_citizen_id = $1
                 AND expires_at > CURRENT_TIMESTAMP
                 AND revoked_at IS NULL
                 ORDER BY created_at DESC",
            )
            .bind(citizen_id)
            .bind(&auth_code_hash)
            .fetch_all(&state.pool)
            .await;
            let sessions = match sessions {
                Ok(sessions) => {
                    debug!(
                        citizen_id,
                        session_count = sessions.len(),
                        "Retrieved active account sessions"
                    );
                    sessions
                }
                Err(error) => {
                    error!(?error, "Failed to fetch account sessions");
                    return render_page(
                        include_str!(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/static/account/account-error.html"
                        )),
                        "Account",
                        jar,
                        &state.pool,
                    )
                    .await
                    .into_response();
                }
            };
            let mut session_items = String::new();
            for session in &sessions {
                let session_uuid: uuid::Uuid = session.get("uuid");
                let device_type: String = session.get("device_type");
                let device_name: String = session.get("device_name");
                let created_at: chrono::DateTime<chrono::Utc> = session.get("created_at");
                let expires_at: chrono::DateTime<chrono::Utc> = session.get("expires_at");
                let current_session: bool = session.get("current_session");
                trace!(%session_uuid, %device_type, current_session, "Rendering account session");
                session_items.push_str(
                    &include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/static/account/session-item.html"
                    ))
                    .replace("$${{device_name}}", &html_escape(&device_name))
                    .replace("$${{device_type}}", &html_escape(&device_type))
                    .replace(
                        "$${{created_at}}",
                        &created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                    )
                    .replace(
                        "$${{expires_at}}",
                        &expires_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                    )
                    .replace(
                        "$${{current_session_hidden}}",
                        if current_session { "" } else { "hidden" },
                    )
                    .replace("$${{session_uuid}}", &session_uuid.to_string()),
                );
            }
            trace!(citizen_id, "Rendering account page");
            render_page(
                &include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/account/account-found.html"
                ))
                .replace("$${{username}}", &html_escape(&username))
                .replace("$${{email}}", &html_escape(&email))
                .replace("$${{display_name}}", &html_escape(&display_name))
                .replace("$${{current_theme}}", theme_name(theme))
                .replace("$${{theme_options}}", &theme_options(theme))
                .replace("$${{session_items}}", &session_items)
                .replace(
                    "$${{sessions_empty_hidden}}",
                    if sessions.is_empty() { "" } else { "hidden" },
                ),
                "Account",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        Ok(None) => {
            warn!("Active session did not resolve to account details");
            render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/account/account-not-found.html"
                )),
                "Account",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        Err(e) => {
            error!("Failed to fetch account info: {}", e);
            render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/account/account-error.html"
                )),
                "Account",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
    }
}

pub async fn post_delete_account_session(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(session_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%session_uuid, "Handling individual session revocation");
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!(%session_uuid, "Redirecting session revocation without a session");
        return Redirect::to("/login").into_response();
    };
    let auth_code_hash = crate::backend::login_oauth::hash_token(&session_token);
    let query = sqlx::query_scalar::<_, bool>(
        "WITH current_session AS (
             SELECT associated_citizen_id
             FROM sessions
             WHERE auth_code_hash = $1
             AND expires_at > CURRENT_TIMESTAMP
             AND revoked_at IS NULL
         )
         UPDATE sessions
         SET revoked_at = CURRENT_TIMESTAMP
         FROM current_session
         WHERE sessions.uuid = $2
         AND sessions.associated_citizen_id = current_session.associated_citizen_id
         AND sessions.revoked_at IS NULL
         RETURNING sessions.auth_code_hash = $1",
    )
    .bind(auth_code_hash)
    .bind(session_uuid)
    .fetch_optional(&state.pool)
    .await;

    match query {
        Ok(Some(true)) => {
            info!(%session_uuid, current_session = true, "Revoked account session");
            (logout_headers(), Redirect::to("/login")).into_response()
        }
        Ok(Some(false)) => {
            info!(%session_uuid, current_session = false, "Revoked account session");
            Redirect::to("/account").into_response()
        }
        Ok(None) => {
            warn!(%session_uuid, "Session revocation found no accessible active session");
            Redirect::to("/account").into_response()
        }
        Err(error) => {
            error!(?error, "Failed to revoke account session");
            let page = render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/account/session-error.html"
                )),
                "Account sessions",
                jar,
                &state.pool,
            )
            .await;
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

pub async fn post_delete_all_account_sessions(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Response {
    trace!("Handling all-session revocation");
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!("Redirecting all-session revocation without a session");
        return Redirect::to("/login").into_response();
    };
    let query = sqlx::query(
        "WITH current_session AS (
             SELECT associated_citizen_id
             FROM sessions
             WHERE auth_code_hash = $1
             AND expires_at > CURRENT_TIMESTAMP
             AND revoked_at IS NULL
         )
         UPDATE sessions
         SET revoked_at = CURRENT_TIMESTAMP
         FROM current_session
         WHERE sessions.associated_citizen_id = current_session.associated_citizen_id
         AND sessions.revoked_at IS NULL",
    )
    .bind(crate::backend::login_oauth::hash_token(&session_token))
    .execute(&state.pool)
    .await;

    match query {
        Ok(result) => {
            info!(
                revoked_session_count = result.rows_affected(),
                "Revoked all account sessions"
            );
            (logout_headers(), Redirect::to("/login")).into_response()
        }
        Err(error) => {
            error!(?error, "Failed to revoke all account sessions");
            let page = render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/account/session-error.html"
                )),
                "Account sessions",
                jar,
                &state.pool,
            )
            .await;
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

pub async fn post_account_theme(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<AccountThemeForm>,
) -> Response {
    trace!(theme = form.theme, "Handling account theme update");
    if form.theme != 0 {
        warn!(theme = form.theme, "Rejected unknown account theme");
        return (
            StatusCode::BAD_REQUEST,
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/errors/unknown-theme.html"
                ))
                .to_string(),
            ),
        )
            .into_response();
    }
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!("Redirecting account theme update without a session");
        return Redirect::to("/login").into_response();
    };
    let query = sqlx::query(
        "INSERT INTO user_setting (user_uuid, setting_key, setting_value, last_updated_by_user_uuid)
         SELECT citizens.uuid, 'theme', $1, citizens.uuid
         FROM sessions JOIN citizens ON citizens.id = sessions.associated_citizen_id
         WHERE sessions.auth_code_hash = $2 AND sessions.expires_at > CURRENT_TIMESTAMP AND sessions.revoked_at IS NULL
         ON CONFLICT (user_uuid, setting_key) DO UPDATE
         SET setting_value = EXCLUDED.setting_value, last_updated_by_user_uuid = EXCLUDED.last_updated_by_user_uuid",
    )
    .bind(form.theme.to_string())
    .bind(crate::backend::login_oauth::hash_token(&session_token))
    .execute(&state.pool)
    .await;

    match query {
        Ok(result) if result.rows_affected() == 1 => {
            info!(theme = form.theme, "Saved account theme");
            (theme_cookie(form.theme), Redirect::to("/account")).into_response()
        }
        Ok(result) => {
            warn!(
                rows_affected = result.rows_affected(),
                "Account theme update did not resolve an active session"
            );
            Redirect::to("/login").into_response()
        }
        Err(error) => {
            error!(?error, "Failed to save account theme");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/static/account/theme-error.html"
                    ))
                    .to_string(),
                ),
            )
                .into_response()
        }
    }
}
