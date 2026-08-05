use crate::backend::login_oauth::refresh_oauth_token;
use crate::pages::login::AppState;
use chrono::{TimeDelta, Utc};
use sqlx::Row;
use tokio::time::{Duration, sleep};
use tracing::{error, info};

pub async fn login_threads(state: AppState) {
    let cleaner_state = state.clone();
    tokio::spawn(async move {
        supervisor_run_clean_login_threads(cleaner_state).await;
    });

    tokio::spawn(async move {
        supervisor_run_refresh_oauth_tokens(state).await;
    });
}

async fn worker_run_clean_login_threads(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match run_clean_login_threads(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_clean_login_threads(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        state
            .pending_logins
            .lock()
            .unwrap()
            .retain(|_, login| Utc::now() - login.created_at <= TimeDelta::seconds(120));

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

async fn worker_run_refresh_oauth_tokens(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match run_refresh_oauth_tokens(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_refresh_oauth_tokens(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let sessions = sqlx::query(
            "SELECT uuid, oauth_refresh_token
            FROM sessions
            WHERE oauth_refresh_token IS NOT NULL
            AND oauth_token_expires_at <= CURRENT_TIMESTAMP + INTERVAL '5 minutes'
            AND expires_at > CURRENT_TIMESTAMP
            AND revoked_at IS NULL",
        )
        .fetch_all(&state.pool)
        .await?;

        for session in sessions {
            let session_uuid: uuid::Uuid = session.get("uuid");
            let refresh_token: Vec<u8> = session.get("oauth_refresh_token");
            let refresh_token = String::from_utf8(refresh_token)?;

            match refresh_oauth_token(&state, &refresh_token).await {
                Ok(token) => {
                    sqlx::query(
                        "UPDATE sessions
                        SET oauth_access_token = $1,
                            oauth_refresh_token = COALESCE($2, oauth_refresh_token),
                            oauth_token_expires_at = $3
                        WHERE uuid = $4",
                    )
                    .bind(token.access_token)
                    .bind(token.refresh_token.map(String::into_bytes))
                    .bind(
                        token
                            .expires_in
                            .map(|seconds| Utc::now() + TimeDelta::seconds(seconds)),
                    )
                    .bind(session_uuid)
                    .execute(&state.pool)
                    .await?;
                }
                Err(error) => {
                    error!(?error, %session_uuid, "Failed to refresh OAuth token");
                }
            }
        }

        sleep(Duration::from_secs(60)).await;
    }
}

async fn supervisor_run_refresh_oauth_tokens(state: AppState) {
    let mut error_count = 0;

    loop {
        let result = tokio::spawn(worker_run_refresh_oauth_tokens(state.clone())).await;
        log_worker_result("OAuth token refresh", result);

        sleep(Duration::from_secs(5)).await;

        info!("Restarting OAuth token refresh worker");
        error_count += 1;

        if error_count >= 10 {
            error!("Too many errors of OAuth token refresh, assuming general failure");
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
