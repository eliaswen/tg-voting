use askama::Template;
use axum::{extract::State, response::IntoResponse};
use axum_extra::extract::cookie::CookieJar;

use crate::pages::login::AppState;
use crate::render::render_template_page;
use tracing::trace;

pub async fn get_about(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling about page request");
    render_template_page(
        &AboutPage {
            version: env!("CARGO_PKG_VERSION"),
        },
        "About",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn get_contact(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling contact page request");
    render_template_page(&ContactPage, "Contact", jar, &state.pool)
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
        return crate::error_handling::error_not_found(State(state), jar)
            .await
            .into_response();
    }
    trace!("Staging page requested in staging mode, returning staging information page");
    render_template_page(&StagingPage, "Staging", jar, &state.pool)
        .await
        .into_response()
}

pub async fn get_issues(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling issues page request");
    render_template_page(&IssuesPage, "Issues", jar, &state.pool)
        .await
        .into_response()
}

#[derive(Template)]
#[template(path = "info/about.html")]
struct AboutPage<'a> {
    version: &'a str,
}

#[derive(Template)]
#[template(path = "info/contact.html")]
struct ContactPage;

#[derive(Template)]
#[template(path = "info/staging.html")]
struct StagingPage;

#[derive(Template)]
#[template(path = "info/issues.html")]
struct IssuesPage;