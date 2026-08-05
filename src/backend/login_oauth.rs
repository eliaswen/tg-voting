use crate::pages::login::{AppState, PendingLoginStatus};
use chrono::{TimeDelta, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::error;

pub(crate) const SESSION_LIFETIME_DAYS: i64 = 30;

#[derive(Deserialize)]
pub(crate) struct OAuthToken {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct OAuthUser {
    sub: String,
    preferred_username: Option<String>,
    email: Option<String>,
    name: Option<String>,
}

pub(crate) async fn handle_oauth_callback(state: AppState, request_id: uuid::Uuid, code: String) {
    let status = match complete_oauth_login(&state, &code).await {
        Ok(session_token) => PendingLoginStatus::Complete(session_token),
        Err(error) => {
            error!(?error, "OAuth login failed");
            PendingLoginStatus::Failed("oauth-failure".to_string())
        }
    };

    if let Some(login) = state.pending_logins.lock().unwrap().get_mut(&request_id) {
        login.status = status;
    }
}

pub(crate) async fn complete_oauth_login(
    state: &AppState,
    code: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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

    let user = state
        .http_client
        .get(&state.oauth_userinfo_url)
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<OAuthUser>()
        .await?;

    let session_token = uuid::Uuid::new_v4().to_string();
    let mut transaction = state.pool.begin().await?;
    let citizen_id = sqlx::query_scalar::<_, i64>(
        "SELECT citizen_id FROM authentik_identities WHERE issuer = $1 AND subject = $2",
    )
    .bind(&state.oauth_issuer)
    .bind(&user.sub)
    .fetch_optional(&mut *transaction)
    .await?
    .unwrap_or(
        sqlx::query_scalar("INSERT INTO citizens DEFAULT VALUES RETURNING id")
            .fetch_one(&mut *transaction)
            .await?,
    );

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

    sqlx::query(
        "INSERT INTO sessions (associated_citizen_id, auth_code_hash, expires_at, oauth_access_token, oauth_refresh_token, oauth_token_expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(citizen_id)
    .bind(hash_token(&session_token))
    .bind(Utc::now() + TimeDelta::days(SESSION_LIFETIME_DAYS))
    .bind(token.access_token.into_bytes())
    .bind(token.refresh_token.map(String::into_bytes))
    .bind(token.expires_in.map(|seconds| Utc::now() + TimeDelta::seconds(seconds)))
    .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(session_token)
}

pub(crate) async fn refresh_oauth_token(
    state: &AppState,
    refresh_token: &str,
) -> Result<OAuthToken, Box<dyn std::error::Error + Send + Sync>> {
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
    let mut form = vec![
        ("client_id", state.oauth_client_id.as_str()),
        ("client_secret", state.oauth_client_secret.as_str()),
    ];
    form.extend_from_slice(parameters);
    Ok(state
        .http_client
        .post(&state.oauth_token_url)
        .form(&form)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub(crate) fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
