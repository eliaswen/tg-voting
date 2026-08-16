use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use tracing::debug;

use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Template)]
#[template(path = "errors/error.html")]
pub struct ErrorPage<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub page_class: &'a str,
    pub back_url: &'a str,
    pub back_label: &'a str,
    pub message_kind: u8,
    pub social_help: bool,
    pub back_period: bool,
}

impl<'a> ErrorPage<'a> {
    pub const fn new(title: &'a str, message: &'a str, page_class: &'a str) -> Self {
        Self {
            title,
            message,
            page_class,
            back_url: "",
            back_label: "",
            message_kind: 0,
            social_help: false,
            back_period: false,
        }
    }

    pub const fn permission(title: &'a str, page_class: &'a str) -> Self {
        Self {
            title,
            message: "",
            page_class,
            back_url: "",
            back_label: "",
            message_kind: 1,
            social_help: false,
            back_period: false,
        }
    }

    pub const fn with_message_kind(mut self, kind: u8) -> Self {
        self.message_kind = kind;
        self
    }

    pub const fn with_social_help(mut self) -> Self {
        self.social_help = true;
        self
    }

    pub const fn with_back(mut self, url: &'a str, label: &'a str) -> Self {
        self.back_url = url;
        self.back_label = label;
        self
    }

    pub const fn with_back_period(mut self) -> Self {
        self.back_period = true;
        self
    }
}

pub fn error_response(status: StatusCode, page: &ErrorPage<'_>) -> Response {
    let html = page
        .render()
        .expect("the compile-time checked error page must render");
    (
        status,
        crate::render::render_public_fallback(&html, page.title),
    )
        .into_response()
}

pub async fn themed_error_response(
    status: StatusCode,
    page: &ErrorPage<'_>,
    state: &AppState,
    jar: CookieJar,
) -> Response {
    (
        status,
        render_template_page(page, page.title, jar, &state.pool).await,
    )
        .into_response()
}

pub async fn error_method(State(state): State<AppState>, jar: CookieJar) -> Response {
    debug!(status = 405, "Returning method not allowed response");
    themed_error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        &ErrorPage::new("405 Method Not Allowed", "", "method-not-allowed-page")
            .with_message_kind(3),
        &state,
        jar,
    )
    .await
}

pub async fn error_not_found(State(state): State<AppState>, jar: CookieJar) -> Response {
    debug!(status = 404, "Returning not found response");
    themed_error_response(
        StatusCode::NOT_FOUND,
        &ErrorPage::new("404 Not Found", "", "not-found-page").with_message_kind(2),
        &state,
        jar,
    )
    .await
}
