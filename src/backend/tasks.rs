use crate::backend::login_oauth::refresh_oauth_token;
use crate::pages::login::AppState;
use chrono::{TimeDelta, Utc};
use sqlx::postgres::PgListener;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, trace, warn};

pub async fn login_threads(state: AppState) {
    info!("Starting login background task supervisors");
    let notification_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = run_login_notifications(&notification_state).await {
                error!(?error, "PostgreSQL OAuth login notification listener stopped");
            }
            sleep(Duration::from_secs(5)).await;
            warn!("Restarting PostgreSQL OAuth login notification listener");
        }
    });

    let cleaner_state = state.clone();
    tokio::spawn(async move {
        debug!("Login cleaner supervisor task started");
        supervisor_run_clean_login_threads(cleaner_state).await;
    });

    let refresh_state = state.clone();
    tokio::spawn(async move {
        debug!("OAuth refresh supervisor task started");
        supervisor_run_refresh_oauth_tokens(refresh_state).await;
    });
    tokio::spawn(async move {
        run_election_snapshots(state).await;
    });
    debug!("Login background task supervisors spawned");
}

async fn run_login_notifications(
    state: &AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut listener = PgListener::connect_with(&state.pool).await?;
    listener.listen("oauth_login_updates").await?;
    info!("Listening for PostgreSQL OAuth login notifications");
    loop {
        let notification = listener.recv().await?;
        match notification.payload().parse::<uuid::Uuid>() {
            Ok(request_id) => {
                let _ = state.login_updates.send(request_id);
                trace!(%request_id, "Relayed PostgreSQL OAuth login notification");
            }
            Err(error) => {
                warn!(?error, payload = notification.payload(), "Ignored malformed OAuth login notification");
            }
        }
    }
}

async fn run_election_snapshots(state: AppState) {
    loop {
        match sqlx::query_scalar::<_, uuid::Uuid>("SELECT uuid FROM elections WHERE status = 'upcoming' AND eligibility_snapshotted_at IS NULL AND registration_starts_at <= CURRENT_TIMESTAMP").fetch_all(&state.pool).await {
            Ok(elections) => for election_uuid in elections { if crate::pages::voting::ensure_snapshot(&state, election_uuid).await.is_err() { error!(%election_uuid, "Failed to snapshot election eligibility"); } },
            Err(error) => error!(?error, "Failed to find elections requiring an eligibility snapshot"),
        }
        crate::election_results::count_due_elections(&state).await;
        sleep(Duration::from_secs(30)).await;
    }
}

async fn worker_run_clean_login_threads(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("Login cleaner worker started");
    match run_clean_login_threads(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_clean_login_threads(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let result = sqlx::query(
            "DELETE FROM pending_oauth_logins
             WHERE expires_at < CURRENT_TIMESTAMP - INTERVAL '10 seconds'",
        )
        .execute(&state.pool)
        .await?;
        if result.rows_affected() > 0 {
            debug!(removed = result.rows_affected(), "Removed expired pending logins");
        } else {
            trace!("Login cleaner cycle completed");
        }

        sleep(Duration::from_secs(1)).await;
    }
}

async fn supervisor_run_clean_login_threads(state: AppState) {
    info!("Login cleaner supervisor started");
    let mut error_count = 0;

    loop {
        let result = tokio::spawn(worker_run_clean_login_threads(state.clone())).await;
        log_worker_result("Login thread cleaner", result);

        sleep(Duration::from_secs(5)).await;

        warn!(
            attempt = error_count + 1,
            "Restarting login thread cleaner worker"
        );
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
    debug!("OAuth token refresh worker started");
    match run_refresh_oauth_tokens(state).await {
        Ok(()) => Err("unexpected exit".into()),
        Err(error) => Err(error),
    }
}

async fn run_refresh_oauth_tokens(
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        trace!("Looking for OAuth tokens requiring refresh");
        let sessions = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT uuid
            FROM sessions
            WHERE oauth_refresh_token IS NOT NULL
            AND oauth_token_expires_at <= CURRENT_TIMESTAMP + INTERVAL '5 minutes'
            AND expires_at > CURRENT_TIMESTAMP
            AND revoked_at IS NULL",
        )
        .fetch_all(&state.pool)
        .await?;
        debug!(
            session_count = sessions.len(),
            "Loaded sessions requiring OAuth token refresh"
        );

        for session_uuid in sessions {
            trace!(%session_uuid, "Refreshing OAuth token for session");
            let mut transaction = state.pool.begin().await?;
            let claimed = sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_xact_lock(hashtextextended($1::text, 0))",
            )
            .bind(session_uuid)
            .fetch_one(&mut *transaction)
            .await?;
            if !claimed {
                trace!(%session_uuid, "Another instance is refreshing the OAuth token");
                transaction.rollback().await?;
                continue;
            }
            let refresh_token = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT oauth_refresh_token
                 FROM sessions
                 WHERE uuid = $1
                   AND oauth_refresh_token IS NOT NULL
                   AND oauth_token_expires_at <= CURRENT_TIMESTAMP + INTERVAL '5 minutes'
                   AND expires_at > CURRENT_TIMESTAMP
                   AND revoked_at IS NULL
                 FOR UPDATE",
            )
            .bind(session_uuid)
            .fetch_optional(&mut *transaction)
            .await?;
            let Some(refresh_token) = refresh_token else {
                trace!(%session_uuid, "OAuth token was already refreshed by another instance");
                transaction.rollback().await?;
                continue;
            };
            let refresh_token = String::from_utf8(refresh_token)?;

            match refresh_oauth_token(&state, &refresh_token).await {
                Ok(token) => {
                    let result = sqlx::query(
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
                    .execute(&mut *transaction)
                    .await?;
                    transaction.commit().await?;
                    info!(%session_uuid, rows_affected = result.rows_affected(), "Refreshed OAuth token for session");
                }
                Err(error) => {
                    error!(?error, %session_uuid, "Failed to refresh OAuth token");
                    transaction.rollback().await?;
                }
            }
        }

        trace!(sleep_seconds = 60, "OAuth token refresh cycle completed");
        sleep(Duration::from_secs(60)).await;
    }
}

async fn supervisor_run_refresh_oauth_tokens(state: AppState) {
    info!("OAuth token refresh supervisor started");
    let mut error_count = 0;

    loop {
        let result = tokio::spawn(worker_run_refresh_oauth_tokens(state.clone())).await;
        log_worker_result("OAuth token refresh", result);

        sleep(Duration::from_secs(5)).await;

        warn!(
            attempt = error_count + 1,
            "Restarting OAuth token refresh worker"
        );
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
