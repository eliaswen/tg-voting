use axum::{extract::State, response::IntoResponse};
use axum_extra::extract::cookie::CookieJar;

use crate::pages::login::AppState;
use crate::render::render_page;
use tracing::trace;

pub async fn get_about(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling about page request");
    render_page(
        &include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/info/about.html"
        ))
        .replace("$${{version}}", env!("CARGO_PKG_VERSION")),
        "About",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn get_contact(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling contact page request");
    render_page(
        &include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/info/contact.html"
        )),
        "Contact",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn get_staging(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling staging page request");
    if state.app_mode != 1 {
        trace!(
            app_mode = state.app_mode,
            "Staging page requested outside staging mode, returning 404"
        );
        return crate::error_handling::error_not_found()
            .await
            .into_response();
    }
    trace!("Staging page requested in staging mode, returning staging information page");
    render_page(
        &include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/info/staging.html"
        )),
        "Staging",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}
