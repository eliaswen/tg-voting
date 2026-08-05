use axum::{
    BoxError,
    extract::{FromRef, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        Html, IntoResponse, Redirect,
        sse::{Event, KeepAlive, Sse},
    },
};
use chrono::{DateTime, Utc};
use futures_util::stream::{self, Stream};
use reqwest::Client;
use serde::Deserialize;
use sqlx::{Row, postgres::PgPool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, sleep};
use tracing::error;

use crate::backend::login_oauth::{
    SESSION_LIFETIME_DAYS, handle_oauth_callback, hash_token as backend_hash_token,
};
use axum_extra::extract::cookie::CookieJar;

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
pub struct OAuthCallbackParams {
    pub code: Option<String>,
    pub state: Option<uuid::Uuid>,
    pub error: Option<String>,
}

#[derive(Clone, FromRef)]
pub struct AppState {
    pub pool: PgPool,
    #[from_ref(skip)]
    pub pending_logins: Arc<Mutex<HashMap<uuid::Uuid, PendingLogin>>>,
    #[from_ref(skip)]
    pub oauth_client_id: String,
    #[from_ref(skip)]
    pub oauth_client_secret: String,
    #[from_ref(skip)]
    pub oauth_authorize_url: String,
    #[from_ref(skip)]
    pub oauth_token_url: String,
    #[from_ref(skip)]
    pub oauth_userinfo_url: String,
    #[from_ref(skip)]
    pub oauth_issuer: String,
    #[from_ref(skip)]
    pub oauth_scope: String,
    #[from_ref(skip)]
    pub public_host: String,
    #[from_ref(skip)]
    pub http_client: Client,
}

pub async fn get_login() -> Html<String> {
    Html(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/login/login.html"
        ))
        .to_string(),
    )
}

pub async fn get_login_oauth(State(state): State<AppState>) -> Redirect {
    let redirect_uri = format!("{}/login/oauth/callback", state.public_host);
    let request_id = uuid::Uuid::now_v7();
    state.pending_logins.lock().unwrap().insert(
        request_id,
        PendingLogin {
            status: PendingLoginStatus::Pending,
            created_at: Utc::now(),
        },
    );

    let auth_url = format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}",
        state.oauth_authorize_url,
        urlencoding::encode(&state.oauth_client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&state.oauth_scope),
        request_id,
    );

    Redirect::temporary(&auth_url)
}

pub async fn get_login_oauth_callback(
    State(state): State<AppState>,
    Query(params): Query<OAuthCallbackParams>,
) -> Html<String> {
    let Some(request_id) = params.state else {
        return login_error_page("session-error");
    };
    let Some(code) = params.code else {
        return login_error_page(if params.error.is_some() {
            "oauth-failure"
        } else {
            "session-error"
        });
    };
    if !state
        .pending_logins
        .lock()
        .unwrap()
        .contains_key(&request_id)
    {
        return login_error_page("session-error");
    }
    tokio::spawn(handle_oauth_callback(state, request_id, code));

    Html(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/login/oauth-callback.html"
        ))
        .replace("$${{request_id}}", request_id.to_string().as_str())
        .to_string(),
    )
}

pub async fn get_login_oauth_status(
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
                Some(PendingLoginStatus::Complete(_)) => {
                    return Ok(Event::default()
                        .event("redirect")
                        .data(format!("/login/oauth/complete/{request_id}")));
                }
                Some(PendingLoginStatus::Failed(error)) => {
                    return Ok(Event::default().event("error").data(error));
                }
                None => return Ok(Event::default().event("error").data("session-error")),
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn get_login_oauth_manual_check(
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
            <meta http-equiv=\"refresh\" content=\"2; url=/login/oauth/manual-check/{request_id}\">"
        ))
        .into_response(),
        Some(PendingLoginStatus::Complete(_)) => {
            Redirect::to(&format!("/login/oauth/complete/{request_id}")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => login_error_page(&error).into_response(),
        None => login_error_page("session-error").into_response(),
    }
}

pub async fn get_login_oauth_complete(
    State(state): State<AppState>,
    Path(request_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    let login = state.pending_logins.lock().unwrap().remove(&request_id);

    match login.map(|login| login.status) {
        Some(PendingLoginStatus::Complete(session_token)) => {
            let mut headers = HeaderMap::new();
            let secure = if state.public_host.starts_with("https://") {
                "; Secure"
            } else {
                ""
            };
            let cookie = format!(
                "session={session_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
                SESSION_LIFETIME_DAYS * 24 * 60 * 60,
                secure,
            );
            headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
            (headers, Redirect::to("/")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => login_error_page(&error).into_response(),
        _ => login_error_page("session-error").into_response(),
    }
}

pub async fn get_userinfo(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    let Some(session_token) = jar.get("session").map(|c| c.value().to_string()) else {
        return Redirect::to("/login").into_response();
    };

    let auth_code_hash = backend_hash_token(&session_token);
    let citizen = sqlx::query(
        "SELECT authentik_identities.preferred_username, authentik_identities.email, authentik_identities.display_name
        FROM sessions
        JOIN authentik_identities ON authentik_identities.citizen_id = sessions.associated_citizen_id
        WHERE sessions.auth_code_hash = $1
        AND sessions.expires_at > CURRENT_TIMESTAMP
        AND sessions.revoked_at IS NULL"
    )
    .bind(auth_code_hash)
    .fetch_optional(&state.pool)
    .await;

    match citizen {
        Ok(Some(citizen)) => {
            let username: Option<String> = citizen.get("preferred_username");
            let email: Option<String> = citizen.get("email");
            let display_name: Option<String> = citizen.get("display_name");
            let identity = display_name.or(username).or(email).unwrap_or_default();
            Html(format!(
                "<h1>User info</h1><p>{}</p>",
                html_escape(&identity)
            ))
            .into_response()
        }
        Ok(None) => Redirect::to("/login").into_response(),
        Err(error) => {
            error!(?error, "Failed to retrieve user info");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<h1>Could not retrieve user info</h1>"),
            )
                .into_response()
        }
    }
}

pub async fn get_login_reddit() -> Html<String> {
    Html("Reddit login not implemented yet".to_string())
}

fn login_error_page(error: &str) -> Html<String> {
    let message = match error {
        "oauth-failure" => "The identity provider has rejected your login.",
        "session-error" => "Could not retrieve your session.",
        _ => "An unexpected error occurred.",
    };

    Html(format!(
        "<p>{message} Please <a href=\"/login\">try again</a>.</p>"
    ))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
