use axum::{
    Form,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::{debug, trace, warn};

use crate::pages::login::AppState;
use crate::render::{render_page, theme_name, theme_options};

#[derive(Deserialize)]
pub struct ThemeForm {
    theme: u8,
}

pub async fn get_settings(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling settings page request");
    let theme = jar
        .get("theme")
        .and_then(|cookie| cookie.value().parse().ok())
        .unwrap_or(0);
    debug!(theme, "Rendering local settings");
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/settings/settings.html"
    ))
    .replace("$${{current_theme}}", theme_name(theme))
    .replace("$${{theme_options}}", &theme_options(theme));
    render_page(&content, "Settings", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_settings(Form(form): Form<ThemeForm>) -> Response {
    trace!(theme = form.theme, "Handling local theme update");
    if form.theme != 0 {
        warn!(theme = form.theme, "Rejected unknown local theme");
        return (
            StatusCode::BAD_REQUEST,
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/errors/unknown-theme.html"
                ))
                .to_string(),
            ),
        )
            .into_response();
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
