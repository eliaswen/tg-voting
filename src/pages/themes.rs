use crate::pages::login::AppState;
use crate::render::render_template_page;
use askama::Template;
use axum::{extract::State, response::IntoResponse};
use axum_extra::extract::cookie::CookieJar;
use tracing::trace;

pub async fn get_list_themes_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> impl IntoResponse {
    trace!("Handling themes page request");
    render_template_page(&ThemesPage, "Themes", jar, &state.pool)
        .await
        .into_response()
}

#[derive(Template)]
#[template(path = "themes/list-themes.html")]
struct ThemesPage;
