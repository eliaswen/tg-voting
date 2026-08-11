use crate::render::render_page;
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
use tracing::{debug, error, info, trace, warn};

use crate::backend::login_oauth::{
    SESSION_LIFETIME_DAYS, handle_oauth_callback, hash_token as backend_hash_token,
    poll_device_login, request_device_authorization,
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
    pub expires_at: DateTime<Utc>,
    pub device: SessionDevice,
}

#[derive(Clone, Debug)]
pub struct SessionDevice {
    pub device_type: String,
    pub device_name: String,
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
    pub oauth_device_authorization_url: Option<String>,
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
    #[from_ref(skip)]
    // app_mode: 0 = development, 1 = staging, 2 = production
    pub app_mode: u8,
}

pub async fn get_login(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!(
        session_present = jar.get("session").is_some(),
        "Handling login page request"
    );

    let session_token = match jar.get("session").map(|c| c.value().to_string()) {
        Some(token) => token,
        None => {
            debug!("Rendering login page for anonymous request");
            return render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/login/login.html"
                )),
                "Login",
                jar,
                &state.pool,
            )
            .await
            .into_response();
        }
    };

    let auth_code_hash = backend_hash_token(&session_token);

    let query = sqlx::query(
        "SELECT 1
        FROM sessions
        WHERE auth_code_hash = $1
        AND expires_at > CURRENT_TIMESTAMP
        AND revoked_at IS NULL",
    )
    .bind(auth_code_hash)
    .fetch_optional(&state.pool)
    .await;

    match query {
        Ok(Some(_)) => {
            debug!("Redirecting active session away from login page");
            return Redirect::to("/").into_response();
        }
        Ok(None) => {
            debug!("Session cookie was not active; rendering login page");
        }
        Err(e) => {
            error!("Failed to fetch session info: {}", e);
        }
    }

    return render_page(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/login/login.html"
        )),
        "Login",
        jar,
        &state.pool,
    )
    .await
    .into_response();
}

pub async fn get_login_oauth(State(state): State<AppState>, headers: HeaderMap) -> Redirect {
    trace!("Starting browser OAuth login");
    let redirect_uri = format!("{}/login/oauth/callback", state.public_host);
    let request_id = uuid::Uuid::now_v7();
    let device = session_device(&headers);
    state.pending_logins.lock().unwrap().insert(
        request_id,
        PendingLogin {
            status: PendingLoginStatus::Pending,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::TimeDelta::seconds(120),
            device: device.clone(),
        },
    );
    debug!(%request_id, device_type = %device.device_type, device_name = %device.device_name, "Registered pending browser OAuth login");

    let auth_url = format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}",
        state.oauth_authorize_url,
        urlencoding::encode(&state.oauth_client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&state.oauth_scope),
        request_id,
    );

    trace!(%request_id, "Redirecting browser OAuth login to identity provider");
    Redirect::temporary(&auth_url)
}

pub async fn get_login_oauth_device(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> impl IntoResponse {
    trace!("Starting OAuth device login");
    let authorization = match request_device_authorization(&state).await {
        Ok(authorization) => authorization,
        Err(error) => {
            error!(?error, "Failed to start OAuth device login");
            let page = render_page(
                &include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/login/device-unavailable.html"
                ))
                .replace(
                    "$${{message}}",
                    "The identity provider is not configured for device login yet.",
                ),
                "Device login unavailable",
                jar,
                &state.pool,
            )
            .await;
            return (StatusCode::SERVICE_UNAVAILABLE, page).into_response();
        }
    };
    if !valid_verification_uri(&authorization.verification_uri)
        || authorization
            .verification_uri_complete
            .as_deref()
            .is_some_and(|uri| !valid_verification_uri(uri))
    {
        error!("OAuth provider returned an invalid device verification URI");
        let page = render_page(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/login/device-unavailable.html"
            ))
            .replace(
                "$${{message}}",
                "The identity provider returned an invalid verification address.",
            ),
            "Device login unavailable",
            jar,
            &state.pool,
        )
        .await;
        return (StatusCode::BAD_GATEWAY, page).into_response();
    }

    let request_id = uuid::Uuid::now_v7();
    let expires_at = Utc::now() + chrono::TimeDelta::seconds(authorization.expires_in);
    let device = session_device(&headers);
    state.pending_logins.lock().unwrap().insert(
        request_id,
        PendingLogin {
            status: PendingLoginStatus::Pending,
            created_at: Utc::now(),
            expires_at,
            device: device.clone(),
        },
    );
    debug!(%request_id, %expires_at, device_type = %device.device_type, device_name = %device.device_name, "Registered pending OAuth device login");
    trace!(%request_id, "Spawning OAuth device login polling task");
    tokio::spawn(poll_device_login(
        state.clone(),
        request_id,
        authorization.device_code,
        authorization.interval.unwrap_or(5),
        expires_at,
    ));

    let verification_uri_complete = authorization.verification_uri_complete.unwrap_or_default();
    let verification_uri_complete_hidden = if verification_uri_complete.is_empty() {
        "hidden"
    } else {
        ""
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/login/oauth-device.html"
    ))
    .replace(
        "$${{verification_uri}}",
        &html_escape(&authorization.verification_uri),
    )
    .replace("$${{user_code}}", &html_escape(&authorization.user_code))
    .replace(
        "$${{verification_uri_complete}}",
        &html_escape(&verification_uri_complete),
    )
    .replace(
        "$${{verification_uri_complete_hidden}}",
        verification_uri_complete_hidden,
    )
    .replace("$${{request_id}}", &request_id.to_string());

    let page = render_page(&content, "Login with another device", jar, &state.pool).await;
    trace!(%request_id, "Rendered OAuth device login page");
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        page,
    )
        .into_response()
}

pub async fn get_login_oauth_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<OAuthCallbackParams>,
) -> Html<String> {
    trace!(request_id = ?params.state, provider_error = params.error.is_some(), authorization_code_present = params.code.is_some(), "Handling OAuth callback");
    let Some(request_id) = params.state else {
        warn!("OAuth callback did not contain state");
        return render_page(
            &login_error_content("session-error"),
            "Login error",
            jar,
            &state.pool,
        )
        .await;
    };
    let Some(code) = params.code else {
        let error = if params.error.is_some() {
            "oauth-failure"
        } else {
            "session-error"
        };
        warn!(%request_id, provider_error = params.error.is_some(), "OAuth callback did not contain an authorization code");
        return render_page(&login_error_content(error), "Login error", jar, &state.pool).await;
    };
    if !state
        .pending_logins
        .lock()
        .unwrap()
        .contains_key(&request_id)
    {
        warn!(%request_id, "OAuth callback referenced an unknown pending login");
        return render_page(
            &login_error_content("session-error"),
            "Login error",
            jar,
            &state.pool,
        )
        .await;
    }
    debug!(%request_id, "OAuth callback matched pending login");
    trace!(%request_id, "Spawning OAuth callback completion task");
    tokio::spawn(handle_oauth_callback(state.clone(), request_id, code));

    render_page(
        &include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/login/oauth-callback.html"
        ))
        .replace("$${{request_id}}", request_id.to_string().as_str())
        .to_string(),
        "Signing in",
        jar,
        &state.pool,
    )
    .await
}

pub async fn get_login_oauth_status(
    State(state): State<AppState>,
    Path(request_id): Path<uuid::Uuid>,
) -> Sse<impl Stream<Item = Result<Event, BoxError>>> {
    trace!(%request_id, "Opening OAuth status event stream");
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
                    debug!(%request_id, "OAuth status event stream observed completed login");
                    return Ok(Event::default()
                        .event("redirect")
                        .data(format!("/login/oauth/complete/{request_id}")));
                }
                Some(PendingLoginStatus::Failed(error)) => {
                    warn!(%request_id, error_code = %error, "OAuth status event stream observed failed login");
                    return Ok(Event::default().event("error").data(error));
                }
                None => {
                    warn!(%request_id, "OAuth status event stream lost pending login");
                    return Ok(Event::default().event("error").data("session-error"));
                }
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn get_login_oauth_manual_check(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(request_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    trace!(%request_id, "Handling manual OAuth status check");
    let status = state
        .pending_logins
        .lock()
        .unwrap()
        .get(&request_id)
        .map(|login| login.status.clone());

    match status {
        Some(PendingLoginStatus::Pending) => {
            trace!(%request_id, "Manual OAuth status check is still pending");
            render_page(
                &include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/login/manual-check.html"
                ))
                .replace("$${{request_id}}", &request_id.to_string()),
                "Signing in",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        Some(PendingLoginStatus::Complete(_)) => {
            debug!(%request_id, "Manual OAuth status check observed completed login");
            Redirect::to(&format!("/login/oauth/complete/{request_id}")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => {
            warn!(%request_id, error_code = %error, "Manual OAuth status check observed failed login");
            render_page(
                &login_error_content(&error),
                "Login error",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        None => {
            warn!(%request_id, "Manual OAuth status check referenced unknown login");
            render_page(
                &login_error_content("session-error"),
                "Login error",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
    }
}

pub async fn get_login_oauth_complete(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(request_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    trace!(%request_id, "Completing OAuth login response");
    let login = state.pending_logins.lock().unwrap().remove(&request_id);

    match login.map(|login| login.status) {
        Some(PendingLoginStatus::Complete(session_token)) => {
            debug!(%request_id, "Issuing session cookie for completed OAuth login");
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
            let theme = sqlx::query_scalar::<_, String>(
                "SELECT user_setting.setting_value
                 FROM sessions
                 JOIN citizens ON citizens.id = sessions.associated_citizen_id
                 JOIN user_setting ON user_setting.user_uuid = citizens.uuid AND user_setting.setting_key = 'theme'
                 WHERE sessions.auth_code_hash = $1",
            )
            .bind(backend_hash_token(&session_token))
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
            if let Some(theme) = theme.filter(|theme| theme == "0") {
                trace!(%request_id, theme = %theme, "Applying saved account theme to login response");
                headers.append(
                    header::SET_COOKIE,
                    HeaderValue::from_str(&format!(
                        "theme={theme}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000"
                    ))
                    .unwrap(),
                );
            }
            info!(%request_id, "OAuth login completed");
            (headers, Redirect::to("/")).into_response()
        }
        Some(PendingLoginStatus::Failed(error)) => {
            warn!(%request_id, error_code = %error, "OAuth login completion received failed state");
            render_page(
                &login_error_content(&error),
                "Login error",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        _ => {
            warn!(%request_id, "OAuth login completion did not find a completed login");
            render_page(
                &login_error_content("session-error"),
                "Login error",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
    }
}

pub async fn get_logout(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!(
        session_present = jar.get("session").is_some(),
        "Handling logout"
    );
    if let Some(session_token) = jar.get("session") {
        match sqlx::query(
            "UPDATE sessions SET revoked_at = CURRENT_TIMESTAMP WHERE auth_code_hash = $1",
        )
        .bind(backend_hash_token(session_token.value()))
        .execute(&state.pool)
        .await
        {
            Ok(result) => info!(
                rows_affected = result.rows_affected(),
                "Revoked session during logout"
            ),
            Err(error) => error!(?error, "Failed to revoke session during logout"),
        }
    } else {
        debug!("Logout request did not include a session");
    }
    (logout_headers(), Redirect::to("/")).into_response()
}

pub fn logout_headers() -> HeaderMap {
    trace!("Building logout cookie headers");
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("theme=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    headers
}

pub async fn get_userinfo(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!(
        session_present = jar.get("session").is_some(),
        "Handling user info request"
    );
    let Some(session_token) = jar.get("session").map(|c| c.value().to_string()) else {
        debug!("Redirecting user info request without a session");
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
            debug!("Retrieved user info for active session");
            let username: Option<String> = citizen.get("preferred_username");
            let email: Option<String> = citizen.get("email");
            let display_name: Option<String> = citizen.get("display_name");
            let identity = display_name.or(username).or(email).unwrap_or_default();
            render_page(
                &include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/login/userinfo.html"
                ))
                .replace("$${{identity}}", &html_escape(&identity)),
                "User info",
                jar,
                &state.pool,
            )
            .await
            .into_response()
        }
        Ok(None) => {
            debug!("User info session was not active");
            Redirect::to("/login").into_response()
        }
        Err(error) => {
            error!(?error, "Failed to retrieve user info");
            let page = render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/login/userinfo-error.html"
                )),
                "User info",
                jar,
                &state.pool,
            )
            .await;
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

fn login_error_content(error: &str) -> String {
    trace!(error_code = error, "Rendering login error content");
    let message = match error {
        "oauth-failure" => "The identity provider has rejected your login.",
        "oauth-access-denied" => "The device login request was declined.",
        "oauth-expired" => "The device login code expired.",
        "session-error" => "Could not retrieve your session.",
        _ => "An unexpected error occurred.",
    };

    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/login/login-error.html"
    ))
    .replace("$${{message}}", message)
}

fn valid_verification_uri(uri: &str) -> bool {
    let valid = reqwest::Url::parse(uri)
        .map(|uri| matches!(uri.scheme(), "http" | "https"))
        .unwrap_or(false);
    trace!(valid, "Validated OAuth verification URI");
    valid
}

fn session_device(headers: &HeaderMap) -> SessionDevice {
    trace!(
        user_agent_present = headers.contains_key(header::USER_AGENT),
        "Identifying session device"
    );
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let device_type = if user_agent.contains("iPad") || user_agent.contains("Tablet") {
        "Tablet"
    } else if user_agent.contains("Mobile")
        || user_agent.contains("iPhone")
        || user_agent.contains("Android")
    {
        "Mobile"
    } else if user_agent.contains("Windows")
        || user_agent.contains("Macintosh")
        || user_agent.contains("Linux")
        || user_agent.contains("X11")
    {
        "Desktop"
    } else {
        "Unknown"
    };
    let browser = if user_agent.contains("Edg/") {
        "Edge"
    } else if user_agent.contains("Firefox/") || user_agent.contains("FxiOS/") {
        "Firefox"
    } else if user_agent.contains("Chrome/") || user_agent.contains("CriOS/") {
        "Chrome"
    } else if user_agent.contains("Safari/") {
        "Safari"
    } else {
        "Unknown browser"
    };
    let operating_system = if user_agent.contains("Windows") {
        "Windows"
    } else if user_agent.contains("Android") {
        "Android"
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        "iOS"
    } else if user_agent.contains("Macintosh") {
        "macOS"
    } else if user_agent.contains("Linux") || user_agent.contains("X11") {
        "Linux"
    } else {
        "unknown operating system"
    };

    let device = SessionDevice {
        device_type: device_type.to_string(),
        device_name: format!("{browser} on {operating_system}"),
    };
    debug!(device_type = %device.device_type, device_name = %device.device_name, "Identified session device");
    device
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(user_agent: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(user_agent).unwrap(),
        );
        headers
    }

    #[test]
    fn identifies_desktop_browser() {
        let device = session_device(&headers(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36",
        ));

        assert_eq!(device.device_type, "Desktop");
        assert_eq!(device.device_name, "Chrome on Windows");
    }

    #[test]
    fn identifies_mobile_browser() {
        let device = session_device(&headers(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 Version/17.5 Mobile/15E148 Safari/604.1",
        ));

        assert_eq!(device.device_type, "Mobile");
        assert_eq!(device.device_name, "Safari on iOS");
    }
}
