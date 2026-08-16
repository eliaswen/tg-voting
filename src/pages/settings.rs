use askama::Template;
use axum::{
    Form,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::{debug, trace, warn};

use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::login::AppState;
use crate::render::{render_template_page, theme_name};

#[derive(Deserialize)]
pub struct ThemeForm {
    theme: u8,
}

#[derive(Deserialize)]
pub struct TimezoneForm {
    timezone: String,
    return_to: String,
}

pub async fn get_settings(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling settings page request");
    let theme = jar
        .get("theme")
        .and_then(|cookie| cookie.value().parse().ok())
        .unwrap_or(1);
    debug!(theme, "Rendering local settings");
    let timezone = crate::render::timezone(&jar);
    let page = SettingsPage {
        timezone: &timezone,
        current_theme: theme_name(theme),
        selected_theme: theme,
        themes: &[(0, "Basic"), (1, "white-simple"), (2, "black-simple")],
    };
    render_template_page(&page, "Settings", jar, &state.pool)
        .await
        .into_response()
}

#[derive(Template)]
#[template(path = "settings/settings.html")]
struct SettingsPage<'a> {
    timezone: &'a str,
    current_theme: &'a str,
    selected_theme: u8,
    themes: &'a [(u8, &'static str)],
}

pub async fn post_settings(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ThemeForm>,
) -> Response {
    trace!(theme = form.theme, "Handling local theme update");
    if form.theme > 2 {
        warn!(theme = form.theme, "Rejected unknown local theme");
        return themed_error_response(
            StatusCode::BAD_REQUEST,
            &ErrorPage::new("Unknown theme", "", "unknown-theme-page").with_message_kind(4),
            &state,
            jar,
        )
        .await;
    }
    debug!(theme = form.theme, "Updated local theme cookie");
    (theme_cookie(form.theme), Redirect::to("/settings")).into_response()
}

pub fn theme_cookie(theme: u8) -> HeaderMap {
    trace!(theme, "Building theme cookie");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "theme={theme}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000"
        ))
        .unwrap(),
    );
    headers
}

pub async fn post_timezone(Form(form): Form<TimezoneForm>) -> Response {
    let timezone = if crate::render::valid_timezone(&form.timezone) {
        form.timezone
    } else {
        "UTC".to_string()
    };
    let return_to = if form.return_to.starts_with('/') && !form.return_to.starts_with("//") {
        form.return_to
    } else {
        "/".to_string()
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "timezone={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000",
            urlencoding::encode(&timezone)
        ))
        .unwrap(),
    );
    (headers, Redirect::to(&return_to)).into_response()
}
