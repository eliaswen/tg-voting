use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::Row;
use tracing::{debug, error, trace};

use crate::pages::auth::current_citizen;
use crate::pages::login::AppState;
use crate::render::render_template_page;

pub async fn get_homepage(State(state): State<AppState>, jar: CookieJar) -> Response {
    trace!("Handling homepage request");
    let (account_banned, account_logged_in, display_name) =
        match current_citizen(&state, &jar).await {
            Ok(Some(citizen)) if citizen.banned => {
                debug!(
                    citizen_id = citizen.id,
                    "Rendering banned account homepage state"
                );
                (true, false, String::new())
            }
            Ok(Some(citizen)) => {
                debug!(
                    citizen_id = citizen.id,
                    "Rendering authenticated homepage state"
                );
                (false, true, citizen.display_name)
            }
            Ok(None) => {
                debug!("Rendering anonymous homepage state");
                (false, false, String::new())
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
    let (has_election, election_uuid, season, election_name, election_status, staging) =
        if let Some(election) = elections.first() {
            let uuid: uuid::Uuid = election.get("uuid");
            trace!(election_uuid = %uuid, "Rendering latest election homepage state");
            (
                true,
                uuid.to_string(),
                election.get::<i32, _>("season").to_string(),
                election.get("name"),
                election.get("status"),
                state.app_mode == 1,
            )
        } else {
            trace!("Rendering homepage without a visible election");
            (
                false,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                state.app_mode == 1,
            )
        };
    let page = HomepagePage {
        account_banned,
        account_logged_in,
        display_name: &display_name,
        has_election,
        election_uuid: &election_uuid,
        season: &season,
        election_name: &election_name,
        election_status: &election_status,
        staging,
    };
    trace!("Rendering homepage");
    render_template_page(&page, "Home", jar, &state.pool)
        .await
        .into_response()
}

#[derive(Template)]
#[template(path = "homepage/homepage.html")]
struct HomepagePage<'a> {
    account_banned: bool,
    account_logged_in: bool,
    display_name: &'a str,
    has_election: bool,
    election_uuid: &'a str,
    season: &'a str,
    election_name: &'a str,
    election_status: &'a str,
    staging: bool,
}
