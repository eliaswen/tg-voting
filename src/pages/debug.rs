use crate::pages::auth::html_escape;
use crate::pages::login::AppState;
use crate::render::render_page;
use crate::{error_method, error_not_found};
use axum::{
    extract::{Form, Path, State},
    response::{Html, IntoResponse},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Row, postgres::PgPool};
use tracing::{debug, error, info, trace, warn};

#[derive(Deserialize)]
pub struct SqlQueryForm {
    query: String,
}

async fn list_tables(pool: &PgPool) -> Html<String> {
    trace!("Retrieving database table list for debug page");
    let query =
        sqlx::query("SELECT schemaname, tablename FROM pg_tables ORDER BY schemaname, tablename;")
            .fetch_all(pool)
            .await;

    match query {
        Ok(rows) => {
            debug!(table_count = rows.len(), "Retrieved database table list");
            let mut tables = String::new();
            for row in rows {
                let schema: &str = row.try_get("schemaname").unwrap_or("");
                let table: &str = row.try_get("tablename").unwrap_or("");
                tables.push_str(
                    &include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/static/debug/list-item.html"
                    ))
                    .replace("$${{value}}", &html_escape(&format!("{schema} - {table}"))),
                );
            }
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/debug/tables.html"
                ))
                .replace("$${{tables}}", &tables),
            )
        }
        Err(e) => {
            error!("Failed to list tables: {}", e);
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/debug/error.html"
                ))
                .replace("$${{message}}", "Failed to list tables."),
            )
        }
    }
}

fn get_sql_query() -> Html<String> {
    trace!("Rendering debug SQL form");
    Html(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/debug/sql.html"
        ))
        .replace("$${{result}}", ""),
    )
}

async fn post_sql_query(pool: &PgPool, content: String) -> Html<String> {
    info!(
        query_length = content.len(),
        "Executing query from debug SQL page"
    );
    let result = sqlx::query(sqlx::AssertSqlSafe(content.as_str()))
        .fetch_all(pool)
        .await;

    match result {
        Ok(result) => {
            info!(row_count = result.len(), "Debug SQL query completed");
            let rows = result
                .iter()
                .map(|row| {
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/static/debug/list-item.html"
                    ))
                    .replace("$${{value}}", &html_escape(&format!("{row:?}")))
                })
                .collect::<String>();
            let result = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/debug/sql-result.html"
            ))
            .replace("$${{rows}}", &rows);
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/debug/sql.html"
                ))
                .replace("$${{result}}", &result),
            )
        }
        Err(error) => {
            warn!(?error, "Debug SQL query failed");
            let result = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/debug/sql-error.html"
            ))
            .replace("$${{error}}", &html_escape(&error.to_string()));
            Html(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/debug/sql.html"
                ))
                .replace("$${{result}}", &result),
            )
        }
    }
}

pub async fn get_debug(
    State(state): State<AppState>,
    Path(path): Path<String>,
    cookies: CookieJar,
) -> impl IntoResponse {
    trace!(%path, "Handling debug GET request");
    match path.as_str() {
        "sql" => get_sql_query().into_response(),
        "tables" => list_tables(&state.pool).await.into_response(),
        "login-threads" => get_login_threads_debug(state).await.into_response(),
        "theme-test" => get_theme_test(state, cookies).await.into_response(),
        _ => {
            debug!(%path, "Unknown debug GET path");
            error_not_found().await.into_response()
        }
    }
}

pub async fn post_debug(
    State(pool): State<PgPool>,
    Path(path): Path<String>,
    Form(form): Form<SqlQueryForm>,
) -> impl IntoResponse {
    trace!(%path, "Handling debug POST request");
    match path.as_str() {
        "sql" => post_sql_query(&pool, form.query).await.into_response(),
        "tables" => error_method().await.into_response(),
        "login-threads" => error_method().await.into_response(),
        "theme-test" => error_method().await.into_response(),
        _ => {
            debug!(%path, "Unknown debug POST path");
            error_not_found().await.into_response()
        }
    }
}

async fn get_login_threads_debug(state: AppState) -> Html<String> {
    let pending_logins = state.pending_logins.lock().unwrap();
    debug!(
        pending_login_count = pending_logins.len(),
        "Rendering pending login debug state"
    );
    Html(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/debug/login-threads.html"
        ))
        .replace("$${{count}}", &pending_logins.len().to_string())
        .replace(
            "$${{pending_logins}}",
            &html_escape(&format!("{pending_logins:?}")),
        ),
    )
}

pub async fn get_theme_test(state: AppState, cookies: CookieJar) -> Html<String> {
    trace!("Rendering theme test page");
    render_page(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/debug/theme-test.html"
        )),
        "Theme Test",
        cookies,
        &state.pool,
    )
    .await
}
