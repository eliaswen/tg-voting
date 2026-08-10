use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::pages::auth::html_escape;
use crate::pages::login::AppState;
use crate::render::render_page;

pub async fn get_elections(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling public elections request");
    trace!("Retrieving visible elections");
    let elections = match sqlx::query(
        "SELECT uuid, season, name, status::text AS status,
                to_char(registration_starts_at, 'YYYY-MM-DD HH24:MI TZ') AS registration_starts_at,
                to_char(registration_ends_at, 'YYYY-MM-DD HH24:MI TZ') AS registration_ends_at,
                to_char(voting_starts_at, 'YYYY-MM-DD HH24:MI TZ') AS voting_starts_at,
                to_char(voting_ends_at, 'YYYY-MM-DD HH24:MI TZ') AS voting_ends_at
         FROM elections WHERE status <> 'draft' ORDER BY season DESC",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(elections) => elections,
        Err(error) => {
            error!(?error, "Failed to retrieve elections");
            return render_page(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/static/elections/error.html"
                )),
                "Elections",
                jar,
                &state.pool,
            )
            .await
            .into_response();
        }
    };
    debug!(
        election_count = elections.len(),
        "Retrieved visible elections"
    );

    let mut items = String::new();
    for election in elections {
        let uuid: uuid::Uuid = election.get("uuid");
        let status: String = election.get("status");
        trace!(election_uuid = %uuid, %status, "Rendering election summary");
        let registration = date_range(
            election.get("registration_starts_at"),
            election.get("registration_ends_at"),
        );
        let voting = date_range(
            election.get("voting_starts_at"),
            election.get("voting_ends_at"),
        );
        let registration_hidden = if status == "registration" {
            ""
        } else {
            "hidden"
        };
        items.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/elections/election-item.html"
            ))
            .replace("$${{election_uuid}}", &uuid.to_string())
            .replace(
                "$${{season}}",
                &election.get::<i32, _>("season").to_string(),
            )
            .replace("$${{name}}", &html_escape(election.get("name")))
            .replace("$${{status}}", &html_escape(&status))
            .replace("$${{status_class}}", &status)
            .replace("$${{registration}}", &registration)
            .replace("$${{voting}}", &voting)
            .replace("$${{registration_hidden}}", registration_hidden),
        );
    }
    let empty_hidden = if items.is_empty() { "" } else { "hidden" };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/elections/elections.html"
    ))
    .replace("$${{election_items}}", &items)
    .replace("$${{empty_hidden}}", empty_hidden);

    trace!("Rendering public elections page");
    render_page(&content, "Elections", jar, &state.pool)
        .await
        .into_response()
}

fn date_range(start: Option<String>, end: Option<String>) -> String {
    match (start, end) {
        (Some(start), Some(end)) => format!("{} to {}", html_escape(&start), html_escape(&end)),
        (Some(start), None) => format!("From {}", html_escape(&start)),
        (None, Some(end)) => format!("Until {}", html_escape(&end)),
        (None, None) => "Not scheduled".to_string(),
    }
}
