use askama::Template;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{TimeDelta, Utc};
use serde::Deserialize;
use tracing::{debug, error, info, trace, warn};

use crate::backend::login_oauth::hash_token;
use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::auth::require_citizen;
use crate::pages::login::AppState;
use crate::render::render_template_page;

const DISCORD_AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
const DISCORD_TOKEN_URL: &str = "https://discord.com/api/v10/oauth2/token";
const DISCORD_CURRENT_USER_URL: &str = "https://discord.com/api/v10/users/@me";

#[derive(Deserialize)]
pub struct DiscordCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DiscordToken {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
    global_name: Option<String>,
}

pub async fn get_discord_link(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling Discord account link request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let (Some(client_id), Some(_)) = (
        state.discord_client_id.as_deref(),
        state.discord_client_secret.as_deref(),
    ) else {
        warn!(
            citizen_id = citizen.id,
            discord_client_id_set = state.discord_client_id.is_some(),
            discord_client_secret_set = state.discord_client_secret.is_some(),
            "Rejected Discord link request because Discord OAuth is not configured"
        );
        warn!(
            "Set DISCORD_CLIENT_ID and DISCORD_CLIENT_SECRET and restart the server to enable Discord account linking"
        );
        return social_error(
            &state,
            jar,
            "Discord account linking is not configured on this server.",
        )
        .await;
    };

    let request_state = uuid::Uuid::new_v4().to_string();
    let expires_at = Utc::now() + TimeDelta::minutes(10);
    let result = sqlx::query(
        "INSERT INTO discord_oauth_requests (citizen_uuid, state_hash, expires_at)
         VALUES ($1, $2, $3)",
    )
    .bind(citizen.uuid)
    .bind(hash_token(&request_state))
    .bind(expires_at)
    .execute(&state.pool)
    .await;
    if let Err(error) = result {
        error!(
            ?error,
            citizen_id = citizen.id,
            "Failed to create Discord OAuth request"
        );
        return social_error(
            &state,
            jar,
            "The Discord link request could not be created.",
        )
        .await;
    }

    let redirect_uri = format!(
        "{}/account/discord/callback",
        state.public_host.trim_end_matches('/')
    );
    let url = format!(
        "{DISCORD_AUTHORIZE_URL}?response_type=code&client_id={}&scope=identify&state={}&redirect_uri={}&prompt=consent",
        urlencoding::encode(client_id),
        urlencoding::encode(&request_state),
        urlencoding::encode(&redirect_uri),
    );
    debug!(citizen_id = citizen.id, %expires_at, "Redirecting citizen to Discord OAuth");
    Redirect::to(&url).into_response()
}

pub async fn get_discord_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<DiscordCallback>,
) -> Response {
    trace!("Handling Discord OAuth callback");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    if let Some(provider_error) = query.error {
        warn!(citizen_id = citizen.id, %provider_error, "Discord OAuth request was rejected");
        return social_error(
            &state,
            jar,
            "Discord did not approve the account link request.",
        )
        .await;
    }
    let (Some(code), Some(request_state)) = (query.code, query.state) else {
        warn!(
            citizen_id = citizen.id,
            "Discord OAuth callback was missing parameters"
        );
        return social_error(
            &state,
            jar,
            "Discord returned an incomplete account link request.",
        )
        .await;
    };
    let (Some(client_id), Some(client_secret)) = (
        state.discord_client_id.as_deref(),
        state.discord_client_secret.as_deref(),
    ) else {
        return social_error(
            &state,
            jar,
            "Discord account linking is not configured on this server.",
        )
        .await;
    };

    let request = sqlx::query_scalar::<_, uuid::Uuid>(
        "UPDATE discord_oauth_requests
         SET used_at = CURRENT_TIMESTAMP
         WHERE citizen_uuid = $1
         AND state_hash = $2
         AND expires_at > CURRENT_TIMESTAMP
         AND used_at IS NULL
         RETURNING uuid",
    )
    .bind(citizen.uuid)
    .bind(hash_token(&request_state))
    .fetch_optional(&state.pool)
    .await;
    match request {
        Ok(Some(_)) => debug!(citizen_id = citizen.id, "Validated Discord OAuth state"),
        Ok(None) => {
            warn!(
                citizen_id = citizen.id,
                "Rejected invalid or expired Discord OAuth state"
            );
            return social_error(
                &state,
                jar,
                "This Discord account link request is invalid or expired.",
            )
            .await;
        }
        Err(error) => {
            error!(
                ?error,
                citizen_id = citizen.id,
                "Failed to validate Discord OAuth state"
            );
            return social_error(
                &state,
                jar,
                "The Discord account link request could not be validated.",
            )
            .await;
        }
    }

    let redirect_uri = format!(
        "{}/account/discord/callback",
        state.public_host.trim_end_matches('/')
    );
    let token = state
        .http_client
        .post(DISCORD_TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await;
    let token = match token {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.json::<DiscordToken>().await {
                Ok(token) => token,
                Err(error) => {
                    error!(?error, "Discord token response could not be read");
                    return social_error(
                        &state,
                        jar,
                        "Discord returned an invalid token response.",
                    )
                    .await;
                }
            },
            Err(error) => {
                error!(?error, "Discord rejected the OAuth token exchange");
                return social_error(&state, jar, "Discord rejected the account link request.")
                    .await;
            }
        },
        Err(error) => {
            error!(?error, "Discord OAuth token request failed");
            return social_error(
                &state,
                jar,
                "Discord could not be reached to finish linking.",
            )
            .await;
        }
    };

    let discord_user = state
        .http_client
        .get(DISCORD_CURRENT_USER_URL)
        .bearer_auth(&token.access_token)
        .send()
        .await;
    let discord_user = match discord_user {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.json::<DiscordUser>().await {
                Ok(user) => user,
                Err(error) => {
                    error!(?error, "Discord user response could not be read");
                    return social_error(
                        &state,
                        jar,
                        "Discord returned invalid account information.",
                    )
                    .await;
                }
            },
            Err(error) => {
                error!(?error, "Discord rejected the current user request");
                return social_error(
                    &state,
                    jar,
                    "Discord did not return your account information.",
                )
                .await;
            }
        },
        Err(error) => {
            error!(?error, "Discord current user request failed");
            return social_error(
                &state,
                jar,
                "Discord could not be reached to retrieve your account.",
            )
            .await;
        }
    };

    let result = sqlx::query(
        "INSERT INTO citizen_discord_links
            (citizen_id, discord_user_id, discord_username, discord_display_name, verified_at)
         VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
         ON CONFLICT (citizen_id) DO UPDATE
         SET discord_user_id = EXCLUDED.discord_user_id,
             discord_username = EXCLUDED.discord_username,
             discord_display_name = EXCLUDED.discord_display_name,
             verified_at = CURRENT_TIMESTAMP",
    )
    .bind(citizen.uuid)
    .bind(&discord_user.id)
    .bind(&discord_user.username)
    .bind(&discord_user.global_name)
    .execute(&state.pool)
    .await;
    match result {
        Ok(_) => {
            info!(citizen_id = citizen.id, discord_user_id = %discord_user.id, "Linked Discord account");
            Redirect::to("/account/social").into_response()
        }
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|error| error.is_unique_violation()) =>
        {
            warn!(citizen_id = citizen.id, discord_user_id = %discord_user.id, "Discord account is already linked");
            social_error(
                &state,
                jar,
                "That Discord account is already linked to another citizen.",
            )
            .await
        }
        Err(error) => {
            error!(
                ?error,
                citizen_id = citizen.id,
                "Failed to save Discord account link"
            );
            social_error(&state, jar, "The Discord account link could not be saved.").await
        }
    }
}

pub async fn post_discord_unlink(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling Discord account unlink request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    match sqlx::query("DELETE FROM citizen_discord_links WHERE citizen_id = $1")
        .bind(citizen.uuid)
        .execute(&state.pool)
        .await
    {
        Ok(result) => {
            info!(
                citizen_id = citizen.id,
                rows_affected = result.rows_affected(),
                "Unlinked Discord account"
            );
            Redirect::to("/account/social").into_response()
        }
        Err(error) => {
            error!(
                ?error,
                citizen_id = citizen.id,
                "Failed to unlink Discord account"
            );
            social_error(&state, jar, "The Discord account could not be unlinked.").await
        }
    }
}

pub async fn get_reddit_link(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling Reddit account link request");
    if let Err(response) = require_citizen(&state, &jar).await {
        return response;
    }
    render_template_page(&RedditLinkPage, "Reddit account linking", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_reddit_unlink(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling Reddit account unlink request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    match sqlx::query("DELETE FROM citizen_reddit_links WHERE citizen_id = $1")
        .bind(citizen.uuid)
        .execute(&state.pool)
        .await
    {
        Ok(result) => {
            info!(
                citizen_id = citizen.id,
                rows_affected = result.rows_affected(),
                "Unlinked Reddit account"
            );
            Redirect::to("/account/social").into_response()
        }
        Err(error) => {
            error!(
                ?error,
                citizen_id = citizen.id,
                "Failed to unlink Reddit account"
            );
            social_error(&state, jar, "The Reddit account could not be unlinked.").await
        }
    }
}

async fn social_error(state: &AppState, jar: CookieJar, message: &str) -> Response {
    themed_error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new(
            "Social Account Link Error",
            message,
            "social-link-error-page",
        )
        .with_social_help()
        .with_back("/account/social", "Return to linked accounts"),
        state,
        jar,
    )
    .await
}

#[derive(Template)]
#[template(path = "account/reddit-wip.html")]
struct RedditLinkPage;
