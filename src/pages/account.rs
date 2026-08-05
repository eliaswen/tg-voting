use crate::pages::login::AppState;
use axum::{extract::State, response::Html};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::error;

pub async fn get_account_page(State(state): State<AppState>, jar: CookieJar) -> Html<String> {
    let session_token = match jar.get("session") {
        Some(cookie) => cookie.value().to_string(),
        None => {
            return Html("<p>No session token found. Please log in.</p>".to_string());
        }
    };

    let auth_code_hash = crate::backend::login_oauth::hash_token(&session_token);

    let query = sqlx::query(
        "SELECT authentik_identities.preferred_username, authentik_identities.email, authentik_identities.display_name
        FROM sessions
        JOIN authentik_identities
        ON authentik_identities.citizen_id = sessions.associated_citizen_id
        WHERE sessions.auth_code_hash = $1"
    )
    .bind(auth_code_hash)
    .fetch_optional(&state.pool)
    .await;

    match query {
        Ok(Some(row)) => {
            let username: String = row.try_get("preferred_username").unwrap_or_default();
            let email: String = row.try_get("email").unwrap_or_default();
            let display_name: String = row.try_get("display_name").unwrap_or_default();
            Html(format!(
                "<h1>Account Page</h1>
                <p>Username: {}</p>
                <p>Email: {}</p>
                <p>Display Name: {}</p>
                <p>If you'd like to edit your account information, do so on <a href=\"https://auth.ewenlau.net/if/user/#/settings\">Authentik</a>.</p>",
                username, email, display_name
            ))
        }
        Ok(None) => Html("<p>Account info not found.</p>".to_string()),
        Err(e) => {
            error!("Failed to fetch account info: {}", e);
            Html("<p>Failed to fetch account info.</p>".to_string())
        }
    }
}
