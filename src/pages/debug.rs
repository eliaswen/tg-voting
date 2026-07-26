use axum::{
    extract::{Form, Path, State},
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use sqlx::postgres::PgPool;
use tracing::error;
use crate::{error_method, error_not_found};

#[derive(Deserialize)]
pub struct SqlQueryForm {
    query: String,
}

async fn list_tables(pool: &PgPool) -> Html<String> {
    let query = sqlx::query!(
        "SELECT schemaname, tablename FROM pg_tables ORDER BY schemaname, tablename;"
    )
    .fetch_all(pool)
    .await;

    match query {
        Ok(rows) => {
            let mut html = "<ul>".to_string();
            for row in rows {
                let schema = row.schemaname.as_deref().unwrap_or("");
                let table = row.tablename.as_deref().unwrap_or("");
                html.push_str(&format!("<li>{} - {}</li>", schema, table));
            }
            html.push_str("</ul>");
            Html(html)
        }
        Err(e) => {
            error!("Failed to list tables: {}", e);
            Html("<p>Failed to list tables.</p>".to_string())
        }
    }
}


fn get_sql_query() -> Html<String> {
    Html(
        "<form method='POST' action='/debug/sql'>
            <input type='text' name='query'>
            <button type='submit'>Submit</button>
        </form>"
            .to_string(),
    )
}

async fn post_sql_query(pool: &PgPool, content: String) -> Html<String> {
    
    let result = sqlx::query(sqlx::AssertSqlSafe(content.as_str())).fetch_all(pool).await;

    match result {
        Ok(result) => Html(format!(
            "<form method='POST' action='/debug/sql'>
            <input type='text' name='query'>
            <button type='submit'>Submit</button>
            </form>
            <br>
            
            <ul>{}</ul>",
            result.iter().map(|row| format!("<li>{:?}</li>", row)).collect::<String>()
        )),
        Err(error) => Html(format!("
        <form method='POST' action='/debug/sql'>
            <input type='text' name='query'>
            <button type='submit'>Submit</button>
            </form>
            <br>
        SQL error: {error}")),
    }
}

pub async fn get_debug(
    State(pool): State<PgPool>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    match path.as_str() {
        "sql" => get_sql_query().into_response(),
        "tables" => list_tables(&pool).await.into_response(),
        _ => error_not_found().await.into_response(),
    }
}

pub async fn post_debug(
    State(pool): State<PgPool>,
    Path(path): Path<String>,
    Form(form): Form<SqlQueryForm>,
) -> impl IntoResponse {
    match path.as_str() {
        "sql" => post_sql_query(&pool, form.query).await.into_response(),
        "tables" => error_method().await.into_response(),
        _ => error_not_found().await.into_response(),
    }
}