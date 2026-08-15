use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::pages::auth::{current_citizen, html_escape};
use crate::pages::login::AppState;
use crate::render::render_page;

pub async fn get_homepage(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling homepage request");
    let (account_banned_hidden, account_logged_in_hidden, account_logged_out_hidden, display_name) =
        match current_citizen(&state, &jar).await {
            Ok(Some(citizen)) if citizen.banned => {
                debug!(
                    citizen_id = citizen.id,
                    "Rendering banned account homepage state"
                );
                ("", "hidden", "hidden", String::new())
            }
            Ok(Some(citizen)) => {
                debug!(
                    citizen_id = citizen.id,
                    "Rendering authenticated homepage state"
                );
                ("hidden", "", "hidden", html_escape(&citizen.display_name))
            }
            Ok(None) => {
                debug!("Rendering anonymous homepage state");
                ("hidden", "hidden", "", String::new())
            }
            Err(response) => return response,
        };

    let elections = match sqlx::query(
        "SELECT uuid, season, name, status::text AS status FROM elections
         WHERE status <> 'draft' ORDER BY season DESC LIMIT 1",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(elections) => {
            debug!(
                election_count = elections.len(),
                "Retrieved latest election for homepage"
            );
            elections
        }
        Err(error) => {
            error!(?error, "Failed to retrieve election for homepage");
            Vec::new()
        }
    };
    let (
        latest_election_hidden,
        no_elections_hidden,
        election_uuid,
        season,
        election_name,
        election_status,
        staging_notice_hidden,
    ) = if let Some(election) = elections.first() {
        let uuid: uuid::Uuid = election.get("uuid");
        trace!(election_uuid = %uuid, "Rendering latest election homepage state");
        (
            "",
            "hidden",
            uuid.to_string(),
            election.get::<i32, _>("season").to_string(),
            html_escape(election.get("name")),
            html_escape(election.get("status")),
            if state.app_mode == 1 { "" } else { "hidden" },
        )
    } else {
        trace!("Rendering homepage without a visible election");
        (
            "hidden",
            "",
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            if state.app_mode == 1 { "" } else { "hidden" },
        )
    };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/homepage/homepage.html"
    ))
    .replace("$${{account_banned_hidden}}", account_banned_hidden)
    .replace("$${{account_logged_in_hidden}}", account_logged_in_hidden)
    .replace("$${{account_logged_out_hidden}}", account_logged_out_hidden)
    .replace("$${{display_name}}", &display_name)
    .replace("$${{latest_election_hidden}}", latest_election_hidden)
    .replace("$${{no_elections_hidden}}", no_elections_hidden)
    .replace("$${{election_uuid}}", &election_uuid)
    .replace("$${{season}}", &season)
    .replace("$${{election_name}}", &election_name)
    .replace("$${{election_status}}", &election_status)
    .replace("$${{staging_notice_hidden}}", staging_notice_hidden);
    trace!("Rendering homepage");
    render_page(&content, "Home", jar, &state.pool)
        .await
        .into_response()
}
