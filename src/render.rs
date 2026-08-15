use axum::response::Html;
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

pub fn theme_name(theme: u8) -> &'static str {
    trace!(theme, "Resolving theme name");
    match theme {
        0 => "Basic",
        1 => "white-simple",
        2 => "black-simple",
        _ => "Unknown",
    }
}

pub fn theme_options(current_theme: u8) -> String {
    trace!(current_theme, "Rendering theme options");
    let option = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/themes/theme-option.html"
    ));
    [(0, "Basic"), (1, "white-simple"), (2, "black-simple")]
        .into_iter()
        .map(|(theme_id, theme_name)| {
            option
                .replace("$${{theme_id}}", &theme_id.to_string())
                .replace("$${{theme_name}}", theme_name)
                .replace(
                    "$${{selected}}",
                    if current_theme == theme_id {
                        "selected"
                    } else {
                        ""
                    },
                )
        })
        .collect()
}

fn show_page_with_theme(
    page_content: &str,
    page_title: &str,
    theme: u8,
    logged_in: bool,
    username: Option<String>,
    management_visible: bool,
) -> Result<Html<String>, Box<dyn std::error::Error>> {
    trace!(page_title, theme, logged_in, "Applying page theme");
    let login_url = if logged_in { "/account" } else { "/login" };

    let login_button_text = if logged_in {
        username.unwrap_or_else(|| "Account".into())
    } else {
        "Login".to_string()
    };

    if theme <= 2 {
        let template = match theme {
            0 => {
                trace!(page_title, "Applied basic theme");
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/themes/basic-theme.html"
                ))
            }
            1 => {
                trace!(page_title, "Applied white-simple theme");
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/themes/white-simple-theme.html"
                ))
            }
            _ => {
                trace!(page_title, "Applied black-simple theme");
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/themes/black-simple-theme.html"
                ))
            }
        };
        Ok(Html(
            template
            .replace("$${{page_content}}", page_content)
            .replace("$${{page_title}}", page_title)
            .replace("$${{login_url}}", login_url)
            .replace("$${{login_button_text}}", &login_button_text)
            .replace(
                "$${{management_navigation}}",
                if management_visible {
                    " | <a id=\"header-management-link\" class=\"navigation-link management-link\" href=\"/manage\">Management</a>"
                } else {
                    ""
                },
            )
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
        .unwrap_or(1);
    let session_present = cookies.get("session").is_some();
    let account = sqlx::query("SELECT COALESCE(NULLIF(authentik_identities.preferred_username, ''), NULLIF(authentik_identities.display_name, ''), 'Account') AS username, citizens.role FROM sessions JOIN citizens ON citizens.uuid = sessions.associated_citizen_id JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid WHERE sessions.auth_code_hash = $1 AND sessions.expires_at > CURRENT_TIMESTAMP AND sessions.revoked_at IS NULL")
        .bind(cookies.get("session").map(|c| crate::backend::login_oauth::hash_token(c.value())).unwrap_or_default())
        .fetch_optional(pool)
        .await;
    let (username, management_visible) = match account {
        Ok(Some(row)) => {
            let username = row.try_get::<Option<String>, _>("username").ok().flatten();
            let role = row.try_get::<i64, _>("role").unwrap_or_default();
            (
                username,
                role & (crate::pages::auth::CENSUS_MINISTER
                    | crate::pages::auth::ELECTION_MINISTER
                    | crate::pages::auth::SUPERADMIN)
                    != 0,
            )
        }
        Ok(None) => (None, false),
        Err(error) => {
            error!(
                ?error,
                page_title, session_present, "Failed to resolve page account state"
            );
            (None, false)
        }
    };
    let logged_in = username.is_some();
    debug!(
        page_title,
        theme, logged_in, session_present, "Resolved page rendering context"
    );

    match show_page_with_theme(
        page_content,
        page_title,
        theme,
        logged_in,
        username,
        management_visible,
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_themes_are_available() {
        assert_eq!(theme_name(1), "white-simple");
        assert_eq!(theme_name(2), "black-simple");
        let options = theme_options(1);
        assert!(options.contains("value=\"0\""));
        assert!(options.contains("value=\"1\" selected"));
        assert!(options.contains("value=\"2\""));
        let black = show_page_with_theme("Content", "Title", 2, false, None, false)
            .unwrap()
            .0;
        assert!(black.contains("black-simple-theme-document"));
    }

    #[test]
    fn management_navigation_is_permission_aware() {
        let visible = show_page_with_theme("Content", "Title", 1, true, None, true)
            .unwrap()
            .0;
        let hidden = show_page_with_theme("Content", "Title", 1, true, None, false)
            .unwrap()
            .0;
        assert!(visible.contains("href=\"/manage\""));
        assert!(!hidden.contains("href=\"/manage\""));
    }
}
