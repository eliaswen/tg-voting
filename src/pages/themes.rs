use crate::pages::login::AppState;
use crate::render::render_page;
use axum::{extract::State, response::IntoResponse};
use axum_extra::extract::cookie::CookieJar;
use tracing::trace;

pub async fn get_list_themes_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> impl IntoResponse {
    trace!("Handling themes page request");
    render_page(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/themes/list-themes.html"
        )),
        "Themes",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}
