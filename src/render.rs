use axum::response::Html;
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

pub fn theme_name(theme: u8) -> &'static str {
    trace!(theme, "Resolving theme name");
    match theme {
        0 => "Basic",
        _ => "Unknown",
    }
}

pub fn theme_options(current_theme: u8) -> String {
    trace!(current_theme, "Rendering theme options");
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/themes/theme-option.html"
    ))
    .replace("$${{theme_id}}", "0")
    .replace("$${{theme_name}}", "Basic")
    .replace(
        "$${{selected}}",
        if current_theme == 0 { "selected" } else { "" },
    )
}

fn show_page_with_theme(
    page_content: &str,
    page_title: &str,
    theme: u8,
    logged_in: bool,
    username: Option<String>,
) -> Result<Html<String>, Box<dyn std::error::Error>> {
    trace!(page_title, theme, logged_in, "Applying page theme");
    let login_url = if logged_in { "/account" } else { "/login" };

    let login_button_text = if logged_in {
        username.unwrap_or_else(|| "Account".into())
    } else {
        "Login".to_string()
    };

    if theme == 0 {
        trace!(page_title, "Applied basic theme");
        Ok(Html(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/themes/basic-theme.html"
            ))
            .replace("$${{page_content}}", page_content)
            .replace("$${{page_title}}", page_title)
            .replace("$${{login_url}}", login_url)
            .replace("$${{login_button_text}}", &login_button_text)
            .to_string(),
        ))
    } else {
        error!("Unknown theme number: {}", theme);
        Err("Unknown theme number".into())
    }
}

pub async fn render_page(
    page_content: &str,
    page_title: &str,
    cookies: CookieJar,
    pool: &sqlx::PgPool,
) -> Html<String> {
    trace!(page_title, "Rendering page");
    let theme = cookies
        .get("theme")
        .and_then(|c| c.value().parse().ok())
        .unwrap_or(0);
    let session_present = cookies.get("session").is_some();
    let username = sqlx::query("SELECT COALESCE(NULLIF(authentik_identities.preferred_username, ''), NULLIF(authentik_identities.display_name, ''), 'Account') FROM sessions JOIN authentik_identities ON authentik_identities.citizen_id = sessions.associated_citizen_id WHERE sessions.auth_code_hash = $1 AND sessions.expires_at > CURRENT_TIMESTAMP AND sessions.revoked_at IS NULL")
        .bind(cookies.get("session").map(|c| crate::backend::login_oauth::hash_token(c.value())).unwrap_or_default())
        .fetch_optional(pool)
        .await;
    let username = match username {
        Ok(row) => row
            .and_then(|row| row.try_get::<Option<String>, _>(0).ok())
            .flatten(),
        Err(error) => {
            error!(
                ?error,
                page_title, session_present, "Failed to resolve page account state"
            );
            None
        }
    };
    let logged_in = username.is_some();
    debug!(
        page_title,
        theme, logged_in, session_present, "Resolved page rendering context"
    );

    match show_page_with_theme(page_content, page_title, theme, logged_in, username) {
        Ok(html) => {
            trace!(page_title, "Finished rendering page");
            html
        }
        Err(e) => {
            error!("Failed to render page with theme: {}", e);
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/errors/theme-render.html"
                ))
                .to_string(),
            )
        }
    }
}
