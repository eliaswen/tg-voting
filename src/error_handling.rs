use axum::{http::StatusCode, response::Html};
use tracing::debug;

pub async fn error_method() -> (StatusCode, Html<&'static str>) {
    debug!(status = 405, "Returning method not allowed response");
    (
        StatusCode::from_u16(405).unwrap(),
        Html(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/errors/method-not-allowed.html"
        ))),
    )
}

pub async fn error_not_found() -> (StatusCode, Html<&'static str>) {
    debug!(status = 404, "Returning not found response");
    (
        StatusCode::from_u16(404).unwrap(),
        Html(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/errors/not-found.html"
        ))),
    )
}
