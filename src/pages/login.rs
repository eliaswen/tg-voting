use axum::{
    extract::{FromRef, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Redirect,
    },
    BoxError,
};
use chrono::{DateTime, TimeDelta, Utc};
use futures_util::stream::{self, Stream};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use tracing::{error, info};

const LOGIN_TIMEOUT_SECONDS: i64 = 120;
const SESSION_LIFETIME_DAYS: i64 = 30;

#[derive(Clone, Debug)]
pub enum PendingLoginStatus {
    Pending,
    Complete(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct PendingLogin {
    pub status: PendingLoginStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct DiscordCallbackParams {
    pub code: String,
}

#[derive(Deserialize)]
struct DiscordToken {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
}

#[derive(Clone, FromRef)]
pub struct AppState {
    pub pool: PgPool,
    #[from_ref(skip)]
    pub pending_logins: Arc<Mutex<HashMap<uuid::Uuid, PendingLogin>>>,
    #[from_ref(skip)]
    pub discord_id: String,
    #[from_ref(skip)]
    pub discord_secret: String,
    #[from_ref(skip)]
    pub public_host: String,
    #[from_ref(skip)]
    pub http_client: Client,
}

pub async fn get_login() -> Html<String> {
    Html(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/login/login.html"
    )).to_string())
}

pub async fn get_login_discord(State(state): State<AppState>) -> Redirect {
    let redirect_uri = format!("{}/login/discord/callback", state.public_host);

    let auth_url = format!(
        "https://discord.com/oauth2/authorize?client_id={}&response_type=code&redirect_uri={}&scope=identify",
        state.discord_id, urlencoding::encode(&redirect_uri)
    );

    Redirect::temporary(&auth_url)
}

pub async fn get_login_discord_callback(
    State(state): State<AppState>,
    Query(params): Query<DiscordCallbackParams>,
) -> Html<String> {
    let request_id = uuid::Uuid::now_v7();

    state.pending_logins.lock().unwrap().insert(request_id, PendingLogin {
        status: PendingLoginStatus::Pending,
        created_at: Utc::now(),
    });

    tokio::spawn(handle_discord_oauth(state, request_id, params.code));

    Html(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/login/discord-callback.html"
    )).replace("$${{request_id}}", request_id.to_string().as_str()).to_string())
}

pub async fn get_login_discord_status(
    State(state): State<AppState>,
    Path(request_id): Path<uuid::Uuid>,
) -> Sse<impl Stream<Item = Result<Event, BoxError>>> {
    let stream = stream::once(async move {
        loop {
            let status = state
                .pending_logins
                .lock()
                .unwrap()
                .get(&request_id)
                .map(|login| login.status.clone());

            match status {
                Some(PendingLoginStatus::Pending) => sleep(Duration::from_millis(250)).await,
                Some(PendingLoginStatus::Complete(_)) => return Ok(
                    Event::default()
                        .event("redirect")
                        .data(format!("/login/discord/complete/{request_id}"))
                ),
                Some(PendingLoginStatus::Failed(error)) => return Ok(
                    Event::default()
                        .event("error")
                        .data(error)
                ),
                None => return Ok(
                    Event::default()
                        .event("error")
                        .data("session-error")
                ),
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn get_login_discord_manual_check(
    State(state): State<AppState>,
    Path(request_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    let status = state
        .pending_logins
        .lock()
        .unwrap()
        .get(&request_id)
        .map(|login| login.status.clone());

    match status {
        Some(PendingLoginStatus::Pending) => Html(format!(
            "<h2>Still signing you in...</h2>
            <p>This page will check again in two seconds.</p>
            <meta http-equiv=\"refresh\" content=\"2; url=/login/discord/manual-check/{request_id}\">"
        )).into_response(),
        Some(PendingLoginStatus::Complete(_)) => {
            Redirect::to(&format!("/login/discord/complete/{request_id}")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => login_error_page(&error).into_response(),
        None => login_error_page("session-error").into_response(),
    }
}

pub async fn get_login_discord_complete(
    State(state): State<AppState>,
    Path(request_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    let login = state.pending_logins.lock().unwrap().remove(&request_id);

    match login.map(|login| login.status) {
        Some(PendingLoginStatus::Complete(session_token)) => {
            let mut headers = HeaderMap::new();
            let secure = if state.public_host.starts_with("https://") { "; Secure" } else { "" };
            let cookie = format!(
                "session={session_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
                SESSION_LIFETIME_DAYS * 24 * 60 * 60,
                secure,
            );
            headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
            (headers, Redirect::to("/userinfo")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => login_error_page(&error).into_response(),
        _ => login_error_page("session-error").into_response(),
    }
}

pub async fn get_userinfo(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(session_token) = get_cookie(&headers, "session") else {
        return Redirect::to("/login").into_response();
    };

    let auth_code_hash = hash_token(session_token);
    let citizen = sqlx::query(
        "SELECT citizens.discord_username
        FROM sessions
        JOIN citizens ON citizens.uuid = sessions.associated_citizen
        WHERE sessions.auth_code_hash = $1
        AND sessions.expires_at > CURRENT_TIMESTAMP
        AND sessions.revoked_at IS NULL"
    )
    .bind(auth_code_hash)
    .fetch_optional(&state.pool)
    .await;

    match citizen {
        Ok(Some(citizen)) => {
            let username: Option<String> = citizen.get("discord_username");
            Html(format!("<h1>User info</h1><p>{}</p>", html_escape(&username.unwrap_or_default()))).into_response()
        }
        Ok(None) => Redirect::to("/login").into_response(),
        Err(error) => {
            error!(?error, "Failed to retrieve user info");
            (StatusCode::INTERNAL_SERVER_ERROR, Html("<h1>Could not retrieve user info</h1>")).into_response()
        }
    }
}

pub async fn get_login_reddit() -> Html<String> {
    Html("Reddit login not implemented yet".to_string())
}

pub async fn login_threads(state: AppState) {
    let cleaner_state = state.clone();
    tokio::spawn(async move {
        supervisor_run_clean_login_threads(cleaner_state).await;
    });

    tokio::spawn(async move {
        supervisor_run_refresh_discord_tokens(state).await;
    });
}

async fn worker_run_clean_login_threads(state: AppState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match run_clean_login_threads(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_clean_login_threads(state: AppState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        state
            .pending_logins
            .lock()
            .unwrap()
            .retain(|_, login| Utc::now() - login.created_at <= TimeDelta::seconds(LOGIN_TIMEOUT_SECONDS));

        sleep(Duration::from_secs(1)).await;
    }
}

async fn supervisor_run_clean_login_threads(state: AppState) {
    let mut error_count = 0;

    loop {
        let result = tokio::spawn(worker_run_clean_login_threads(state.clone())).await;
        log_worker_result("Login thread cleaner", result);

        sleep(Duration::from_secs(5)).await;

        info!("Restarting login thread cleaner worker");
        error_count += 1;

        if error_count >= 10 {
            error!("Too many errors of the login thread cleaner, assuming general failure");
            std::process::exit(1);
        }
    }
}

async fn worker_run_refresh_discord_tokens(state: AppState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match run_refresh_discord_tokens(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_refresh_discord_tokens(state: AppState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let sessions = sqlx::query(
            "SELECT uuid, discord_refresh_token
            FROM sessions
            WHERE discord_refresh_token IS NOT NULL
            AND discord_token_expires_at <= CURRENT_TIMESTAMP + INTERVAL '5 minutes'
            AND expires_at > CURRENT_TIMESTAMP
            AND revoked_at IS NULL"
        )
        .fetch_all(&state.pool)
        .await?;

        for session in sessions {
            let session_uuid: uuid::Uuid = session.get("uuid");
            let refresh_token: String = session.get("discord_refresh_token");

            match refresh_discord_token(&state, &refresh_token).await {
                Ok(token) => {
                    sqlx::query(
                        "UPDATE sessions
                        SET discord_access_token = $1,
                            discord_refresh_token = $2,
                            discord_token_expires_at = $3
                        WHERE uuid = $4"
                    )
                    .bind(token.access_token)
                    .bind(token.refresh_token)
                    .bind(Utc::now() + TimeDelta::seconds(token.expires_in))
                    .bind(session_uuid)
                    .execute(&state.pool)
                    .await?;
                }
                Err(error) => {
                    error!(?error, %session_uuid, "Failed to refresh Discord token");
                }
            }
        }

        sleep(Duration::from_secs(60)).await;
    }
}

async fn supervisor_run_refresh_discord_tokens(state: AppState) {
    let mut error_count = 0;

    loop {
        let result = tokio::spawn(worker_run_refresh_discord_tokens(state.clone())).await;
        log_worker_result("Discord token refresh", result);

        sleep(Duration::from_secs(5)).await;

        info!("Restarting Discord token refresh worker");
        error_count += 1;

        if error_count >= 10 {
            error!("Too many errors of Discord token refresh, assuming general failure");
            std::process::exit(1);
        }
    }
}

fn log_worker_result(
    worker_name: &str,
    result: Result<Result<(), Box<dyn std::error::Error + Send + Sync>>, tokio::task::JoinError>,
) {
    match result {
        Ok(Ok(())) => error!("{worker_name} worker exited unexpectedly without an error"),
        Ok(Err(error)) => error!(?error, "{worker_name} worker returned an error"),
        Err(error) if error.is_panic() => error!(?error, "{worker_name} worker panicked"),
        Err(error) => error!(?error, "{worker_name} worker task was cancelled"),
    }
}

async fn handle_discord_oauth(
    state: AppState,
    request_id: uuid::Uuid,
    code: String,
) {
    let result = complete_discord_oauth(&state, &code).await;

    let status = match result {
        Ok(session_token) => PendingLoginStatus::Complete(session_token),
        Err(error) => {
            error!(?error, "Discord OAuth failed");
            PendingLoginStatus::Failed("discord-failure".to_string())
        }
    };

    if let Some(login) = state.pending_logins.lock().unwrap().get_mut(&request_id) {
        login.status = status;
    }
}

async fn complete_discord_oauth(
    state: &AppState,
    code: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let redirect_uri = format!("{}/login/discord/callback", state.public_host);
    let response = state.http_client
        .post("https://discord.com/api/oauth2/token")
        .form(&[
            ("client_id", state.discord_id.as_str()),
            ("client_secret", state.discord_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?;
    let token: DiscordToken = response.json().await?;

    let user = state.http_client
        .get("https://discord.com/api/users/@me")
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<DiscordUser>()
        .await?;

    let session_token = uuid::Uuid::new_v4().to_string();
    let auth_code_hash = hash_token(&session_token);
    let mut transaction = state.pool.begin().await?;

    let citizen_uuid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO citizens (discord_username, discord_id)
        VALUES ($1, $2)
        ON CONFLICT (discord_id) DO UPDATE
        SET discord_username = EXCLUDED.discord_username
        RETURNING uuid"
    )
    .bind(user.username)
    .bind(user.id)
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO sessions (
            associated_citizen,
            auth_code_hash,
            expires_at,
            discord_access_token,
            discord_refresh_token,
            discord_token_expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(citizen_uuid)
    .bind(auth_code_hash)
    .bind(Utc::now() + TimeDelta::days(SESSION_LIFETIME_DAYS))
    .bind(token.access_token)
    .bind(token.refresh_token)
    .bind(Utc::now() + TimeDelta::seconds(token.expires_in))
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(session_token)
}

async fn refresh_discord_token(
    state: &AppState,
    refresh_token: &str,
) -> Result<DiscordToken, Box<dyn std::error::Error + Send + Sync>> {
    let response = state.http_client
        .post("https://discord.com/api/oauth2/token")
        .form(&[
            ("client_id", state.discord_id.as_str()),
            ("client_secret", state.discord_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?
        .error_for_status()?;

    Ok(response.json().await?)
}

fn get_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{name}=")))
}

fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn login_error_page(error: &str) -> Html<String> {
    let message = match error {
        "discord-failure" => "Discord has rejected your login.",
        "session-error" => "Could not retrieve your session.",
        _ => "An unexpected error occurred.",
    };

    Html(format!("<p>{message} Please <a href=\"/login\">try again</a>.</p>"))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
