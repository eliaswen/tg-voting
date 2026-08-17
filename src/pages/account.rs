use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::login::{AppState, logout_headers};
use crate::pages::settings::theme_cookie;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::{debug, error, info, trace, warn};

#[derive(Deserialize)]
pub struct AccountThemeForm {
    theme: u8,
}

#[derive(Deserialize)]
pub struct AccountRoleForm {
    role: i64,
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
            Redirect::to("/account/sessions").into_response()
        }
        Ok(None) => {
            warn!(%session_uuid, "Session revocation found no accessible active session");
            Redirect::to("/account/sessions").into_response()
        }
        Err(error) => {
            error!(?error, "Failed to revoke account session");
            themed_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new("Could not update sessions", "There was an error while trying to update your sessions. Please try again later.", "session-error-page").with_back("/account", "Return to your account").with_back_period(),
                &state,
                jar,
            ).await
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
            themed_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new("Could not update sessions", "There was an error while trying to update your sessions. Please try again later.", "session-error-page").with_back("/account", "Return to your account").with_back_period(),
                &state,
                jar,
            ).await
        }
    }
}

pub async fn post_account_theme(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<AccountThemeForm>,
) -> Response {
    trace!(theme = form.theme, "Handling account theme update");
    if form.theme > 4 {
        warn!(theme = form.theme, "Rejected unknown account theme");
        return themed_error_response(
            StatusCode::BAD_REQUEST,
            &ErrorPage::new("Unknown theme", "", "unknown-theme-page").with_message_kind(4),
            &state,
            jar,
        )
        .await;
    }
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!("Redirecting account theme update without a session");
        return Redirect::to("/login").into_response();
    };
    let query = sqlx::query(
        "INSERT INTO user_setting (user_uuid, setting_key, setting_value, last_updated_by_user_uuid)
         SELECT citizens.uuid, 'theme', $1, citizens.uuid
         FROM sessions JOIN citizens ON citizens.uuid = sessions.associated_citizen_id
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
            (
                theme_cookie(form.theme),
                Redirect::to("/account/appearance"),
            )
                .into_response()
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
            themed_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new("Could not save theme", "", "account-theme-error-page"),
                &state,
                jar,
            )
            .await
        }
    }
}

pub async fn post_account_role(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<AccountRoleForm>,
) -> Response {
    trace!(role = form.role, "Handling account role update");
    if state.app_mode == 2 {
        warn!(
            role = form.role,
            app_mode = state.app_mode,
            "Rejected account role update outside staging mode"
        );
        return themed_error_response(
            StatusCode::FORBIDDEN,
            &ErrorPage::permission("403 Forbidden", "forbidden-page"),
            &state,
            jar,
        )
        .await;
    }
    let Some(session_token) = jar.get("session").map(|cookie| cookie.value().to_string()) else {
        debug!(
            role = form.role,
            "Redirecting account role update without a session"
        );
        return Redirect::to("/login").into_response();
    };
    let query = sqlx::query(
        "WITH current_session AS (
             SELECT associated_citizen_id
             FROM sessions
             WHERE auth_code_hash = $2
             AND expires_at > CURRENT_TIMESTAMP
             AND revoked_at IS NULL
         )
         UPDATE citizens
         SET role = $1
         FROM current_session
         WHERE citizens.uuid = current_session.associated_citizen_id",
    )
    .bind(form.role)
    .bind(crate::backend::login_oauth::hash_token(&session_token))
    .execute(&state.pool)
    .await;

    match query {
        Ok(result) if result.rows_affected() == 1 => {
            info!(role = form.role, "Saved account role");
            Redirect::to("/account").into_response()
        }
        Ok(result) => {
            warn!(
                rows_affected = result.rows_affected(),
                role = form.role,
                "Account role update did not resolve an active session"
            );
            Redirect::to("/login").into_response()
        }
        Err(error) => {
            error!(?error, "Failed to save account role");
            themed_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &ErrorPage::new(
                    "Account Role Error",
                    "We could not update your role right now.",
                    "account-error-page",
                ),
                &state,
                jar,
            )
            .await
        }
    }
}
