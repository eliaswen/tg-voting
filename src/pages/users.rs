use crate::pages::login::AppState;
use crate::render::render_template_page;
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;

#[derive(Template)]
#[template(path = "users/user.html")]
struct UserPage {
    display_name: String,
    username: String,
    discord_username: String,
    reddit_username: String,
}

pub async fn get_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(user_uuid): Path<uuid::Uuid>,
) -> Response {
    let user = match sqlx::query(
        "SELECT COALESCE(NULLIF(authentik_identities.display_name, ''), NULLIF(authentik_identities.preferred_username, ''), 'Citizen ' || citizens.id::text) AS display_name,
         COALESCE(NULLIF(authentik_identities.preferred_username, ''), 'Not available') AS username,
         COALESCE(NULLIF(citizen_discord_links.discord_username, ''), 'Not linked') AS discord_username,
         COALESCE(NULLIF(citizen_reddit_links.reddit_username, ''), 'Not linked') AS reddit_username
         FROM citizens LEFT JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
         LEFT JOIN citizen_discord_links ON citizen_discord_links.citizen_id = citizens.uuid
         LEFT JOIN citizen_reddit_links ON citizen_reddit_links.citizen_id = citizens.uuid WHERE citizens.uuid = $1"
    ).bind(user_uuid).fetch_optional(&state.pool).await {
        Ok(Some(user)) => user,
        _ => return crate::error_handling::error_not_found(State(state), jar).await.into_response(),
    };
    render_template_page(
        &UserPage {
            display_name: user.get("display_name"),
            username: user.get("username"),
            discord_username: user.get("discord_username"),
            reddit_username: user.get("reddit_username"),
        },
        "User",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}
