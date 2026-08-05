use axum::{http::StatusCode, response::Html};

pub async fn error_method() -> (StatusCode, Html<&'static str>) {
    (
        StatusCode::from_u16(405).unwrap(),
        Html("<h1>405 Method Not Allowed</h1>"),
    )
}

pub async fn error_not_found() -> (StatusCode, Html<&'static str>) {
    (
        StatusCode::from_u16(404).unwrap(),
        Html("<h1>404 Not Found</h1>"),
    )
}
