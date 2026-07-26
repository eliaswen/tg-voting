use axum::response::Html;

pub async fn get_homepage() -> Html<&'static str> {
    Html("<h1>Hello World</h1>")
}