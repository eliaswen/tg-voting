use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::pages::auth::{html_escape, require_citizen};
use crate::pages::login::AppState;
use crate::render::{render_page, theme_name, theme_options};

#[derive(Deserialize, Default)]
pub struct AccountSearch {
    #[serde(default)]
    q: String,
}

pub async fn get_account_overview(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling account overview request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let account = match sqlx::query(
        "SELECT authentik_identities.preferred_username, authentik_identities.email,
                authentik_identities.display_name, citizens.citizen_id, citizens.role
         FROM citizens
         JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
         WHERE citizens.uuid = $1",
    )
    .bind(citizen.uuid)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(account)) => account,
        Ok(None) => return Redirect::to("/login").into_response(),
        Err(error) => return account_error(&state, jar, error).await,
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/account/overview.html"
    ))
    .replace(
        "$${{username}}",
        &html_escape(account.try_get("preferred_username").unwrap_or("")),
    )
    .replace(
        "$${{email}}",
        &html_escape(account.try_get("email").unwrap_or("")),
    )
    .replace(
        "$${{display_name}}",
        &html_escape(account.try_get("display_name").unwrap_or("")),
    )
    .replace(
        "$${{citizen_id}}",
        &html_escape(
            account
                .try_get::<Option<String>, _>("citizen_id")
                .unwrap_or_default()
                .as_deref()
                .unwrap_or("Not assigned"),
        ),
    )
    .replace(
        "$${{role_section}}",
        if state.app_mode == 1 {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/account/role-form.html"
            ))
            .replace(
                "$${{current_role}}",
                &account.get::<i64, _>("role").to_string(),
            )
        } else {
            String::new()
        }
        .as_str(),
    );
    render_page(&content, "Account", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_account_social(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling linked accounts page request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let links = match sqlx::query(
        "SELECT citizen_discord_links.discord_username, citizen_reddit_links.reddit_username
         FROM citizens
         LEFT JOIN citizen_discord_links ON citizen_discord_links.citizen_id = citizens.uuid
         LEFT JOIN citizen_reddit_links ON citizen_reddit_links.citizen_id = citizens.uuid
         WHERE citizens.uuid = $1",
    )
    .bind(citizen.uuid)
    .fetch_one(&state.pool)
    .await
    {
        Ok(links) => links,
        Err(error) => return account_error(&state, jar, error).await,
    };
    let discord_username = links
        .try_get::<Option<String>, _>("discord_username")
        .unwrap_or_default();
    let reddit_username = links
        .try_get::<Option<String>, _>("reddit_username")
        .unwrap_or_default();
    let discord_available =
        state.discord_client_id.is_some() && state.discord_client_secret.is_some();
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/account/social.html"
    ))
    .replace(
        "$${{discord_username}}",
        &html_escape(discord_username.as_deref().unwrap_or("Not linked")),
    )
    .replace(
        "$${{discord_connected_hidden}}",
        if discord_username.is_some() {
            ""
        } else {
            "hidden"
        },
    )
    .replace(
        "$${{discord_disconnected_hidden}}",
        if discord_username.is_none() {
            ""
        } else {
            "hidden"
        },
    )
    .replace(
        "$${{discord_link_hidden}}",
        if discord_username.is_none() && discord_available {
            ""
        } else {
            "hidden"
        },
    )
    .replace(
        "$${{discord_unavailable_hidden}}",
        if discord_available { "hidden" } else { "" },
    )
    .replace(
        "$${{reddit_username}}",
        &html_escape(reddit_username.as_deref().unwrap_or("Not linked")),
    )
    .replace(
        "$${{reddit_connected_hidden}}",
        if reddit_username.is_some() {
            ""
        } else {
            "hidden"
        },
    )
    .replace(
        "$${{reddit_disconnected_hidden}}",
        if reddit_username.is_none() {
            ""
        } else {
            "hidden"
        },
    );
    render_page(&content, "Linked accounts", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_account_appearance(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling account appearance page request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let theme = match sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(user_setting.setting_value, '1')
         FROM citizens
         LEFT JOIN user_setting
            ON user_setting.user_uuid = citizens.uuid
            AND user_setting.setting_key = 'theme'
         WHERE citizens.uuid = $1",
    )
    .bind(citizen.uuid)
    .fetch_one(&state.pool)
    .await
    {
        Ok(theme) => theme.parse().unwrap_or(1),
        Err(error) => return account_error(&state, jar, error).await,
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/account/appearance.html"
    ))
    .replace("$${{current_theme}}", theme_name(theme))
    .replace("$${{theme_options}}", &theme_options(theme));
    render_page(&content, "Account appearance", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_account_sessions(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(search): Query<AccountSearch>,
) -> Response {
    trace!(search = %search.q, "Handling account sessions page request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let current_hash = jar
        .get("session")
        .map(|cookie| crate::backend::login_oauth::hash_token(cookie.value()))
        .unwrap_or_default();
    let pattern = format!("%{}%", search.q.trim());
    let sessions = match sqlx::query(
        "SELECT uuid, COALESCE(device_type, 'Unknown') AS device_type,
                COALESCE(device_name, 'Unknown device') AS device_name,
                created_at, expires_at, auth_code_hash = $2 AS current_session
         FROM sessions
         WHERE associated_citizen_id = $1
         AND expires_at > CURRENT_TIMESTAMP
         AND revoked_at IS NULL
         AND ($3 = '%%' OR COALESCE(device_type, '') ILIKE $3 OR COALESCE(device_name, '') ILIKE $3)
         ORDER BY created_at DESC",
    )
    .bind(citizen.uuid)
    .bind(&current_hash)
    .bind(&pattern)
    .fetch_all(&state.pool)
    .await
    {
        Ok(sessions) => sessions,
        Err(error) => return account_error(&state, jar, error).await,
    };
    let mut session_items = String::new();
    for session in &sessions {
        let session_uuid: uuid::Uuid = session.get("uuid");
        session_items.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/account/session-item.html"
            ))
            .replace(
                "$${{device_name}}",
                &html_escape(session.get("device_name")),
            )
            .replace(
                "$${{device_type}}",
                &html_escape(session.get("device_type")),
            )
            .replace(
                "$${{created_at}}",
                &session
                    .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string(),
            )
            .replace(
                "$${{expires_at}}",
                &session
                    .get::<chrono::DateTime<chrono::Utc>, _>("expires_at")
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string(),
            )
            .replace(
                "$${{current_session_hidden}}",
                if session.get("current_session") {
                    ""
                } else {
                    "hidden"
                },
            )
            .replace("$${{session_uuid}}", &session_uuid.to_string()),
        );
    }
    debug!(
        citizen_id = citizen.id,
        result_count = sessions.len(),
        "Retrieved account sessions"
    );
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/account/sessions.html"
    ))
    .replace("$${{search_query}}", &html_escape(search.q.trim()))
    .replace("$${{session_items}}", &session_items)
    .replace(
        "$${{sessions_empty_hidden}}",
        if sessions.is_empty() { "" } else { "hidden" },
    );
    render_page(&content, "Account sessions", jar, &state.pool)
        .await
        .into_response()
}

async fn account_error(state: &AppState, jar: CookieJar, error: sqlx::Error) -> Response {
    error!(?error, "Failed to retrieve account page data");
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
