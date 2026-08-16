use askama::Template;
use axum::{
    extract::{Form, Path, State},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Row, postgres::PgPool};
use tracing::{debug, info, trace, warn};

use crate::pages::login::AppState;
use crate::render::render_template_page;
use crate::{error_method, error_not_found};

#[derive(Deserialize)]
pub struct SqlQueryForm {
    query: String,
}

#[derive(Template)]
#[template(path = "debug/sql.html")]
struct SqlDebugPage {
    has_result: bool,
    rows: Vec<String>,
    error: String,
}

#[derive(Template)]
#[template(path = "debug/tables.html")]
struct TablesDebugPage {
    tables: Vec<String>,
    error: String,
}

#[derive(Template)]
#[template(path = "debug/login-threads.html")]
struct LoginThreadsDebugPage {
    count: usize,
    pending_logins: String,
}

#[derive(Template)]
#[template(path = "debug/theme-test.html")]
struct ThemeTestPage;

async fn list_tables(pool: &PgPool) -> TablesDebugPage {
    trace!("Retrieving database table list for debug page");
    match sqlx::query("SELECT schemaname, tablename FROM pg_tables ORDER BY schemaname, tablename;")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            debug!(table_count = rows.len(), "Retrieved database table list");
            TablesDebugPage {
                tables: rows
                    .into_iter()
                    .map(|row| {
                        let schema: &str = row.try_get("schemaname").unwrap_or("");
                        let table: &str = row.try_get("tablename").unwrap_or("");
                        format!("{schema} - {table}")
                    })
                    .collect(),
                error: String::new(),
            }
        }
        Err(error) => {
            warn!(?error, "Failed to list tables");
            TablesDebugPage {
                tables: Vec::new(),
                error: "Failed to list tables.".to_string(),
            }
        }
    }
}

async fn post_sql_query(pool: &PgPool, content: String) -> SqlDebugPage {
    info!(
        query_length = content.len(),
        "Executing query from debug SQL page"
    );
    match sqlx::query(sqlx::AssertSqlSafe(content.as_str()))
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            info!(row_count = rows.len(), "Debug SQL query completed");
            SqlDebugPage {
                has_result: true,
                rows: rows.iter().map(|row| format!("{row:?}")).collect(),
                error: String::new(),
            }
        }
        Err(error) => {
            warn!(?error, "Debug SQL query failed");
            SqlDebugPage {
                has_result: true,
                rows: Vec::new(),
                error: error.to_string(),
            }
        }
    }
}

pub async fn get_debug(
    State(state): State<AppState>,
    Path(path): Path<String>,
    cookies: CookieJar,
) -> Response {
    trace!(%path, "Handling debug GET request");
    if state.app_mode != 0 {
        return error_not_found(State(state), cookies).await.into_response();
    }
    match path.as_str() {
        "sql" => render_template_page(
            &SqlDebugPage {
                has_result: false,
                rows: Vec::new(),
                error: String::new(),
            },
            "SQL debug",
            cookies,
            &state.pool,
        )
        .await
        .into_response(),
        "tables" => {
            let page = list_tables(&state.pool).await;
            render_template_page(&page, "Database tables", cookies, &state.pool)
                .await
                .into_response()
        }
        "login-threads" => {
            let page = {
                let pending = state.pending_logins.lock().unwrap();
                LoginThreadsDebugPage {
                    count: pending.len(),
                    pending_logins: format!("{pending:?}"),
                }
            };
            render_template_page(&page, "Login Threads", cookies, &state.pool)
                .await
                .into_response()
        }
        "theme-test" => render_template_page(&ThemeTestPage, "Theme Test", cookies, &state.pool)
            .await
            .into_response(),
        _ => {
            debug!(%path, "Unknown debug GET path");
            error_not_found(State(state), cookies).await.into_response()
        }
    }
}

pub async fn post_debug(
    State(state): State<AppState>,
    Path(path): Path<String>,
    cookies: CookieJar,
    Form(form): Form<SqlQueryForm>,
) -> Response {
    trace!(%path, "Handling debug POST request");
    if state.app_mode != 0 {
        return error_not_found(State(state), cookies).await.into_response();
    }
    match path.as_str() {
        "sql" => {
            let page = post_sql_query(&state.pool, form.query).await;
            render_template_page(&page, "SQL debug", cookies, &state.pool)
                .await
                .into_response()
        }
        "tables" | "login-threads" | "theme-test" => {
            error_method(State(state), cookies).await.into_response()
        }
        _ => error_not_found(State(state), cookies).await.into_response(),
    }
}
