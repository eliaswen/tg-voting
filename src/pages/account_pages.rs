use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::auth::require_citizen;
use crate::pages::login::AppState;
use crate::render::{render_template_page, theme_name};

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
    let page = AccountOverviewPage {
        username: account.try_get("preferred_username").unwrap_or(""),
        email: account.try_get("email").unwrap_or(""),
        display_name: account.try_get("display_name").unwrap_or(""),
        citizen_id: account
            .try_get::<Option<String>, _>("citizen_id")
            .unwrap_or_default()
            .unwrap_or_else(|| "Not assigned".to_string()),
        current_role: account.get("role"),
        staging: state.app_mode != 2,
    };
    render_template_page(&page, "Account", jar, &state.pool)
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
    let page = AccountSocialPage {
        discord_username: discord_username.as_deref().unwrap_or("Not linked"),
        discord_connected: discord_username.is_some(),
        discord_available,
        reddit_username: reddit_username.as_deref().unwrap_or("Not linked"),
        reddit_connected: reddit_username.is_some(),
    };
    render_template_page(&page, "Linked accounts", jar, &state.pool)
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
    let page = AccountAppearancePage {
        current_theme: theme_name(theme),
        selected_theme: theme,
        themes: &[(0, "Basic"), (1, "White"), (2, "Black"), (3, "OLED Black"), (4, "Zeedith's Theme")],
    };
    render_template_page(&page, "Account appearance", jar, &state.pool)
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
    let timezone = crate::render::timezone(&jar);
    let sessions_query = "SELECT uuid, COALESCE(device_type, 'Unknown') AS device_type,
                COALESCE(device_name, 'Unknown device') AS device_name,
                to_char(created_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS created_at,
                to_char(expires_at AT TIME ZONE 'Europe/Paris', 'YYYY-MM-DD HH24:MI') || ' Europe/Paris' AS expires_at,
                auth_code_hash = $2 AS current_session
         FROM sessions
         WHERE associated_citizen_id = $1
         AND expires_at > CURRENT_TIMESTAMP
         AND revoked_at IS NULL
         AND ($3 = '%%' OR COALESCE(device_type, '') ILIKE $3 OR COALESCE(device_name, '') ILIKE $3)
         ORDER BY sessions.created_at DESC".replace("Europe/Paris", &timezone);
    let sessions = match sqlx::query(sqlx::AssertSqlSafe(sessions_query.as_str()))
        .bind(citizen.uuid)
        .bind(&current_hash)
        .bind(&pattern)
        .fetch_all(&state.pool)
        .await
    {
        Ok(sessions) => sessions,
        Err(error) => return account_error(&state, jar, error).await,
    };
    let mut session_items = Vec::new();
    for session in &sessions {
        let session_uuid: uuid::Uuid = session.get("uuid");
        session_items.push(AccountSessionItem {
            uuid: session_uuid,
            device_name: session.get("device_name"),
            device_type: session.get("device_type"),
            created_at: session.get("created_at"),
            expires_at: session.get("expires_at"),
            current: session.get("current_session"),
        });
    }
    debug!(
        citizen_id = citizen.id,
        result_count = sessions.len(),
        "Retrieved account sessions"
    );
    let page = AccountSessionsPage {
        search_query: search.q.trim(),
        sessions: &session_items,
    };
    render_template_page(&page, "Account sessions", jar, &state.pool)
        .await
        .into_response()
}

#[derive(Template)]
#[template(path = "account/overview.html")]
struct AccountOverviewPage<'a> {
    username: &'a str,
    email: &'a str,
    display_name: &'a str,
    citizen_id: String,
    current_role: i64,
    staging: bool,
}

#[derive(Template)]
#[template(path = "account/social.html")]
struct AccountSocialPage<'a> {
    discord_username: &'a str,
    discord_connected: bool,
    discord_available: bool,
    reddit_username: &'a str,
    reddit_connected: bool,
}

#[derive(Template)]
#[template(path = "account/appearance.html")]
struct AccountAppearancePage<'a> {
    current_theme: &'a str,
    selected_theme: u8,
    themes: &'a [(u8, &'static str)],
}

struct AccountSessionItem {
    uuid: uuid::Uuid,
    device_name: String,
    device_type: String,
    created_at: String,
    expires_at: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "account/sessions.html")]
struct AccountSessionsPage<'a> {
    search_query: &'a str,
    sessions: &'a [AccountSessionItem],
}

async fn account_error(state: &AppState, jar: CookieJar, error: sqlx::Error) -> Response {
    error!(?error, "Failed to retrieve account page data");
    themed_error_response(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new("Account Page", "There was an error while trying to retrieve your account information. This error should not be your fault.", "account-error-page").with_message_kind(5),
        state,
        jar,
    ).await
}
