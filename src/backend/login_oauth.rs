use crate::pages::login::{AppState, PendingLoginStatus, SessionDevice};
use chrono::{TimeDelta, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, trace, warn};

pub(crate) const SESSION_LIFETIME_DAYS: i64 = 30;

#[derive(Deserialize)]
pub(crate) struct OAuthToken {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct DeviceAuthorization {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    pub(crate) verification_uri_complete: Option<String>,
    pub(crate) expires_in: i64,
    pub(crate) interval: Option<u64>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
}

#[derive(Deserialize)]
struct OAuthUser {
    sub: String,
    preferred_username: Option<String>,
    email: Option<String>,
    name: Option<String>,
}

pub(crate) async fn handle_oauth_callback(state: AppState, request_id: uuid::Uuid, code: String) {
    trace!(%request_id, "OAuth callback worker started");
    let Some(device) = state
        .pending_logins
        .lock()
        .unwrap()
        .get(&request_id)
        .map(|login| login.device.clone())
    else {
        warn!(%request_id, "OAuth callback worker could not find pending login");
        return;
    };
    debug!(%request_id, device_type = %device.device_type, device_name = %device.device_name, "Loaded pending OAuth callback context");
    let status = match complete_oauth_login(&state, &code, &device).await {
        Ok(session_token) => PendingLoginStatus::Complete(session_token),
        Err(error) => {
            error!(?error, "OAuth login failed");
            PendingLoginStatus::Failed("oauth-failure".to_string())
        }
    };

    if let Some(login) = state.pending_logins.lock().unwrap().get_mut(&request_id) {
        login.status = status;
        debug!(%request_id, "Stored OAuth callback result");
    } else {
        warn!(%request_id, "Pending OAuth login disappeared before callback result could be stored");
    }
}

pub(crate) async fn complete_oauth_login(
    state: &AppState,
    code: &str,
    device: &SessionDevice,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    trace!("Exchanging browser OAuth authorization code");
    let redirect_uri = format!("{}/login/oauth/callback", state.public_host);
    let token = exchange_token(
        state,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ],
    )
    .await?;

    debug!(
        refresh_token_present = token.refresh_token.is_some(),
        expires_in = token.expires_in,
        "Browser OAuth token exchange completed"
    );
    complete_oauth_token(state, token, device).await
}

pub(crate) async fn request_device_authorization(
    state: &AppState,
) -> Result<DeviceAuthorization, Box<dyn std::error::Error + Send + Sync>> {
    trace!("Requesting OAuth device authorization");
    let endpoint = state
        .oauth_device_authorization_url
        .as_ref()
        .ok_or_else(|| {
            std::io::Error::other(
                "The OAuth provider does not advertise a device authorization endpoint",
            )
        })?;
    let authorization = state
        .http_client
        .post(endpoint)
        .form(&[
            ("client_id", state.oauth_client_id.as_str()),
            ("scope", state.oauth_scope.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceAuthorization>()
        .await?;
    debug!(
        expires_in = authorization.expires_in,
        interval = authorization.interval,
        complete_uri_present = authorization.verification_uri_complete.is_some(),
        "Received OAuth device authorization"
    );
    if authorization.expires_in <= 0 || TimeDelta::try_seconds(authorization.expires_in).is_none() {
        warn!(
            expires_in = authorization.expires_in,
            "OAuth provider returned invalid device authorization lifetime"
        );
        return Err(std::io::Error::other(
            "The OAuth provider returned an invalid device code lifetime",
        )
        .into());
    }
    Ok(authorization)
}

pub(crate) async fn poll_device_login(
    state: AppState,
    request_id: uuid::Uuid,
    device_code: String,
    mut interval: u64,
    expires_at: chrono::DateTime<Utc>,
) {
    debug!(%request_id, %expires_at, interval, "OAuth device polling worker started");
    let Some(device) = state
        .pending_logins
        .lock()
        .unwrap()
        .get(&request_id)
        .map(|login| login.device.clone())
    else {
        warn!(%request_id, "OAuth device polling worker could not find pending login");
        return;
    };
    interval = interval.max(1);
    while Utc::now() < expires_at {
        trace!(%request_id, interval, "Waiting before OAuth device token poll");
        sleep(Duration::from_secs(interval)).await;
        if Utc::now() >= expires_at {
            debug!(%request_id, "OAuth device authorization expired before next poll");
            break;
        }
        trace!(%request_id, "Polling OAuth device token endpoint");
        let response = state
            .http_client
            .post(&state.oauth_token_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code.as_str()),
                ("client_id", state.oauth_client_id.as_str()),
                ("client_secret", state.oauth_client_secret.as_str()),
            ])
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                warn!(?error, %request_id, "Device login token poll failed");
                continue;
            }
        };
        if response.status().is_success() {
            debug!(%request_id, "OAuth device token endpoint returned success");
            let status = match response.json::<OAuthToken>().await {
                Ok(token) => match complete_oauth_token(&state, token, &device).await {
                    Ok(session_token) => PendingLoginStatus::Complete(session_token),
                    Err(error) => {
                        error!(?error, %request_id, "Device OAuth login failed");
                        PendingLoginStatus::Failed("oauth-failure".to_string())
                    }
                },
                Err(error) => {
                    error!(?error, %request_id, "Device OAuth token response was invalid");
                    PendingLoginStatus::Failed("oauth-failure".to_string())
                }
            };
            set_pending_login_status(&state, request_id, status);
            return;
        }
        let error = response
            .json::<OAuthError>()
            .await
            .ok()
            .map(|error| error.error);
        match error.as_deref() {
            Some("authorization_pending") => {
                trace!(%request_id, "OAuth device authorization is still pending");
            }
            Some("slow_down") => {
                interval = interval.saturating_add(5);
                warn!(%request_id, interval, "OAuth provider requested slower device polling");
            }
            Some("access_denied") => {
                warn!(%request_id, "OAuth device authorization was denied");
                set_pending_login_status(
                    &state,
                    request_id,
                    PendingLoginStatus::Failed("oauth-access-denied".to_string()),
                );
                return;
            }
            Some("expired_token") => {
                debug!(%request_id, "OAuth provider reported expired device code");
                break;
            }
            Some(error_code) => {
                error!(%error_code, %request_id, "Device OAuth token request was rejected");
                set_pending_login_status(
                    &state,
                    request_id,
                    PendingLoginStatus::Failed("oauth-failure".to_string()),
                );
                return;
            }
            None => {
                error!(%request_id, "Device OAuth token request returned an invalid error response");
                set_pending_login_status(
                    &state,
                    request_id,
                    PendingLoginStatus::Failed("oauth-failure".to_string()),
                );
                return;
            }
        }
    }
    set_pending_login_status(
        &state,
        request_id,
        PendingLoginStatus::Failed("oauth-expired".to_string()),
    );
}

fn set_pending_login_status(state: &AppState, request_id: uuid::Uuid, status: PendingLoginStatus) {
    let status_name = match &status {
        PendingLoginStatus::Pending => "pending",
        PendingLoginStatus::Complete(_) => "complete",
        PendingLoginStatus::Failed(_) => "failed",
    };
    if let Some(login) = state.pending_logins.lock().unwrap().get_mut(&request_id) {
        login.status = status;
        debug!(%request_id, status = status_name, "Updated pending login status");
    } else {
        warn!(%request_id, status = status_name, "Could not update missing pending login status");
    }
}

async fn complete_oauth_token(
    state: &AppState,
    token: OAuthToken,
    device: &SessionDevice,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    trace!(device_type = %device.device_type, device_name = %device.device_name, "Completing OAuth token login");
    trace!("Requesting OAuth user information");
    let user = state
        .http_client
        .get(&state.oauth_userinfo_url)
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<OAuthUser>()
        .await?;
    debug!(
        username_present = user.preferred_username.is_some(),
        email_present = user.email.is_some(),
        display_name_present = user.name.is_some(),
        "Received OAuth user information"
    );

    let session_token = uuid::Uuid::new_v4().to_string();
    trace!("Starting OAuth login database transaction");
    let mut transaction = state.pool.begin().await?;
    let citizen_id = sqlx::query_scalar::<_, i64>(
        "SELECT citizen_id FROM authentik_identities WHERE issuer = $1 AND subject = $2",
    )
    .bind(&state.oauth_issuer)
    .bind(&user.sub)
    .fetch_optional(&mut *transaction)
    .await?;
    let citizen_id = match citizen_id {
        Some(citizen_id) => {
            trace!(citizen_id, "Found citizen for OAuth identity");
            citizen_id
        }
        None => {
            let citizen_id = sqlx::query_scalar("INSERT INTO citizens DEFAULT VALUES RETURNING id")
                .fetch_one(&mut *transaction)
                .await?;
            info!(citizen_id, "Created citizen for OAuth identity");
            citizen_id
        }
    };

    let citizen_id: i64 = sqlx::query_scalar(
        "INSERT INTO authentik_identities (citizen_id, issuer, subject, preferred_username, email, display_name, last_authenticated_at)
         VALUES ($1, $2, $3, $4, $5, $6, CURRENT_TIMESTAMP)
         ON CONFLICT (issuer, subject) DO UPDATE
         SET preferred_username = EXCLUDED.preferred_username,
             email = EXCLUDED.email,
             display_name = EXCLUDED.display_name,
             last_authenticated_at = CURRENT_TIMESTAMP
         RETURNING citizen_id",
    )
    .bind(citizen_id)
    .bind(&state.oauth_issuer)
    .bind(user.sub)
    .bind(user.preferred_username)
    .bind(user.email)
    .bind(user.name)
    .fetch_one(&mut *transaction).await?;
    debug!(citizen_id, "Updated OAuth identity");

    sqlx::query(
        "INSERT INTO sessions (associated_citizen_id, auth_code_hash, expires_at, device_type, device_name, oauth_access_token, oauth_refresh_token, oauth_token_expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(citizen_id)
    .bind(hash_token(&session_token))
    .bind(Utc::now() + TimeDelta::days(SESSION_LIFETIME_DAYS))
    .bind(&device.device_type)
    .bind(&device.device_name)
    .bind(token.access_token.into_bytes())
    .bind(token.refresh_token.map(String::into_bytes))
    .bind(token.expires_in.map(|seconds| Utc::now() + TimeDelta::seconds(seconds)))
    .execute(&mut *transaction).await?;
    transaction.commit().await?;
    info!(citizen_id, device_type = %device.device_type, device_name = %device.device_name, "Created authenticated session");
    Ok(session_token)
}

pub(crate) async fn refresh_oauth_token(
    state: &AppState,
    refresh_token: &str,
) -> Result<OAuthToken, Box<dyn std::error::Error + Send + Sync>> {
    trace!("Exchanging OAuth refresh token");
    exchange_token(
        state,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

async fn exchange_token(
    state: &AppState,
    parameters: &[(&str, &str)],
) -> Result<OAuthToken, Box<dyn std::error::Error + Send + Sync>> {
    let grant_type = parameters
        .iter()
        .find(|(name, _)| *name == "grant_type")
        .map(|(_, value)| *value)
        .unwrap_or("unknown");
    trace!(grant_type, "Sending OAuth token request");
    let mut form = vec![
        ("client_id", state.oauth_client_id.as_str()),
        ("client_secret", state.oauth_client_secret.as_str()),
    ];
    form.extend_from_slice(parameters);
    let token = state
        .http_client
        .post(&state.oauth_token_url)
        .form(&form)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    debug!(grant_type, "OAuth token request completed");
    Ok(token)
}

pub(crate) fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::login::PendingLogin;
    use axum::{Form, Router, http::StatusCode, response::IntoResponse, routing::post};
    use reqwest::Client;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    async fn mock_provider() -> String {
        async fn device(Form(form): Form<HashMap<String, String>>) -> impl IntoResponse {
            assert_eq!(form.get("client_id").map(String::as_str), Some("client"));
            assert_eq!(
                form.get("scope").map(String::as_str),
                Some("openid profile email")
            );
            (
                [("content-type", "application/json")],
                r#"{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":"https://auth.example/device","verification_uri_complete":"https://auth.example/device?code=ABCD-EFGH","expires_in":600,"interval":1}"#,
            )
        }

        async fn token(Form(form): Form<HashMap<String, String>>) -> impl IntoResponse {
            assert_eq!(
                form.get("grant_type").map(String::as_str),
                Some("urn:ietf:params:oauth:grant-type:device_code")
            );
            assert_eq!(
                form.get("device_code").map(String::as_str),
                Some("device-code")
            );
            assert_eq!(form.get("client_id").map(String::as_str), Some("client"));
            assert_eq!(
                form.get("client_secret").map(String::as_str),
                Some("secret")
            );
            (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                r#"{"error":"access_denied"}"#,
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/device", post(device))
                    .route("/token", post(token)),
            )
            .await
            .unwrap();
        });
        format!("http://{address}")
    }

    fn state(provider: &str) -> AppState {
        AppState {
            pool: PgPoolOptions::new()
                .connect_lazy("postgresql://localhost/test")
                .unwrap(),
            pending_logins: Arc::new(Mutex::new(HashMap::new())),
            oauth_client_id: "client".to_string(),
            oauth_client_secret: "secret".to_string(),
            oauth_authorize_url: format!("{provider}/authorize"),
            oauth_device_authorization_url: Some(format!("{provider}/device")),
            oauth_token_url: format!("{provider}/token"),
            oauth_userinfo_url: format!("{provider}/userinfo"),
            oauth_issuer: provider.to_string(),
            oauth_scope: "openid profile email".to_string(),
            public_host: "http://localhost".to_string(),
            http_client: Client::new(),
            app_mode: 2,
        }
    }

    #[tokio::test]
    async fn device_authorization_is_loaded_from_the_provider() {
        let provider = mock_provider().await;
        let authorization = request_device_authorization(&state(&provider))
            .await
            .unwrap();

        assert_eq!(authorization.device_code, "device-code");
        assert_eq!(authorization.user_code, "ABCD-EFGH");
        assert_eq!(authorization.interval, Some(1));
    }

    #[tokio::test]
    async fn device_login_reports_access_denied() {
        let provider = mock_provider().await;
        let state = state(&provider);
        let request_id = uuid::Uuid::now_v7();
        let expires_at = Utc::now() + TimeDelta::seconds(5);
        state.pending_logins.lock().unwrap().insert(
            request_id,
            PendingLogin {
                status: PendingLoginStatus::Pending,
                created_at: Utc::now(),
                expires_at,
                device: SessionDevice {
                    device_type: "Desktop".to_string(),
                    device_name: "Test browser on Test OS".to_string(),
                },
            },
        );

        poll_device_login(
            state.clone(),
            request_id,
            "device-code".to_string(),
            1,
            expires_at,
        )
        .await;

        let status = state
            .pending_logins
            .lock()
            .unwrap()
            .get(&request_id)
            .unwrap()
            .status
            .clone();
        assert!(
            matches!(status, PendingLoginStatus::Failed(error) if error == "oauth-access-denied")
        );
    }
}
