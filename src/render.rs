use askama::Template;
use axum::response::Html;
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

pub fn timezone(cookies: &CookieJar) -> String {
    let value = cookies
        .get("timezone")
        .and_then(|cookie| urlencoding::decode(cookie.value()).ok())
        .map(|value| value.into_owned())
        .unwrap_or_else(|| "UTC".to_string());
    if valid_timezone(&value) {
        value
    } else {
        "UTC".to_string()
    }
}

pub fn valid_timezone(value: &str) -> bool {
    value == "UTC"
        || value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '+')
        }) && std::path::Path::new("/usr/share/zoneinfo")
            .join(value)
            .is_file()
}

pub fn theme_name(theme: u8) -> &'static str {
    trace!(theme, "Resolving theme name");
    match theme {
        0 => "Basic",
        1 => "White",
        2 => "Black",
        3 => "OLED Black",
        4 => "Zeedith",
        _ => "Unknown",
    }
}

#[derive(Template)]
#[template(path = "themes/basic-theme.html")]
struct BasicTheme<'a> {
    page_content: &'a str,
    page_title: &'a str,
    login_url: &'a str,
    login_button_text: &'a str,
    management_visible: bool,
}

#[derive(Template)]
#[template(path = "themes/white-simple-theme.html")]
struct WhiteSimpleTheme<'a> {
    page_content: &'a str,
    page_title: &'a str,
    login_url: &'a str,
    login_button_text: &'a str,
    management_visible: bool,
}

#[derive(Template)]
#[template(path = "themes/black-simple-theme.html")]
struct BlackSimpleTheme<'a> {
    page_content: &'a str,
    page_title: &'a str,
    login_url: &'a str,
    login_button_text: &'a str,
    management_visible: bool,
}

#[derive(Template)]
#[template(path = "themes/oled-black-simple-theme.html")]
struct OLEDBlackSimpleTheme<'a> {
    page_content: &'a str,
    page_title: &'a str,
    login_url: &'a str,
    login_button_text: &'a str,
    management_visible: bool,
}

#[derive(Template)]
#[template(path = "themes/zeedith-theme-1.html")]
struct ZeedithTheme1<'a> {
    page_content: &'a str,
    page_title: &'a str,
    login_url: &'a str,
    login_button_text: &'a str,
    management_visible: bool,
}

#[derive(Template)]
#[template(path = "errors/error.html")]
struct ThemeRenderError {
    title: &'static str,
    message: &'static str,
    page_class: &'static str,
    back_url: &'static str,
    back_label: &'static str,
    message_kind: u8,
    social_help: bool,
    back_period: bool,
}

impl ThemeRenderError {
    const fn page() -> Self {
        Self {
            title: "Error",
            message: "",
            page_class: "theme-render-error-page",
            back_url: "/settings",
            back_label: "Return to settings",
            message_kind: 6,
            social_help: false,
            back_period: false,
        }
    }
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

    if theme <= 4 {
        let rendered = match theme {
            0 => {
                trace!(page_title, "Applied basic theme");
                BasicTheme {
                    page_content,
                    page_title,
                    login_url,
                    login_button_text: &login_button_text,
                    management_visible,
                }
                .render()
            }
            1 => {
                trace!(page_title, "Applied white-simple theme");
                WhiteSimpleTheme {
                    page_content,
                    page_title,
                    login_url,
                    login_button_text: &login_button_text,
                    management_visible,
                }
                .render()
            }
            2 => {
                trace!(page_title, "Applied black-simple theme");
                BlackSimpleTheme {
                    page_content,
                    page_title,
                    login_url,
                    login_button_text: &login_button_text,
                    management_visible,
                }
                .render()
            }
            3 => {
                trace!(page_title, "Applied oled-black-simple theme");
                OLEDBlackSimpleTheme {
                    page_content,
                    page_title,
                    login_url,
                    login_button_text: &login_button_text,
                    management_visible,
                }
                .render()
            }
            4 => {
                trace!(page_title, "Applied zeedith-theme-1 theme");
                ZeedithTheme1 {
                    page_content,
                    page_title,
                    login_url,
                    login_button_text: &login_button_text,
                    management_visible,
                }
                .render()
            }
            _ => unreachable!(),
        };
        Ok(Html(rendered?))
    } else {
        error!("Unknown theme number: {}", theme);
        Err("Unknown theme number".into())
    }
}

pub fn render_public_fallback(page_content: &str, page_title: &str) -> Html<String> {
    show_page_with_theme(page_content, page_title, 1, false, None, false).unwrap_or_else(|_| {
        Html(
            ThemeRenderError::page()
                .render()
                .expect("the fallback error template must render"),
        )
    })
}

async fn render_page_context(
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
        Ok(mut html) => {
            if cookies.get("timezone").is_none() {
                html.0 = html.0.replace(
                    "</body>",
                    "<script>const form=document.createElement('form');form.method='post';form.action='/settings/timezone';const timezone=document.createElement('input');timezone.type='hidden';timezone.name='timezone';timezone.value=Intl.DateTimeFormat().resolvedOptions().timeZone||'UTC';const returnTo=document.createElement('input');returnTo.type='hidden';returnTo.name='return_to';returnTo.value=window.location.pathname+window.location.search;form.append(timezone,returnTo);document.body.append(form);form.submit()</script></body>",
                );
            }
            trace!(page_title, "Finished rendering page");
            html
        }
        Err(e) => {
            error!("Failed to render page with theme: {}", e);
            Html(
                ThemeRenderError::page()
                    .render()
                    .expect("the fallback error template must render"),
            )
        }
    }
}

pub async fn render_template_page<T: Template>(
    page: &T,
    page_title: &str,
    cookies: CookieJar,
    pool: &sqlx::PgPool,
) -> Html<String> {
    match page.render() {
        Ok(content) => render_page_context(&content, page_title, cookies, pool).await,
        Err(error) => {
            error!(?error, page_title, "Failed to render Askama page template");
            Html(
                ThemeRenderError::page()
                    .render()
                    .expect("the fallback error template must render"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_themes_are_available() {
        assert_eq!(theme_name(1), "White");
        assert_eq!(theme_name(2), "Black");
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
