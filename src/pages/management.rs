use askama::Template;
use axum::{extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::cookie::CookieJar;
use tracing::{debug, trace};

use crate::error_handling::{ErrorPage, themed_error_response};
use crate::pages::auth::{CENSUS_MINISTER, ELECTION_MINISTER, SUPERADMIN, require_citizen};
use crate::pages::login::AppState;
use crate::render::render_template_page;

pub async fn get_management(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    trace!("Handling management page request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let election_visible = citizen.role & (ELECTION_MINISTER | SUPERADMIN) != 0;
    let census_visible = citizen.role & (CENSUS_MINISTER | SUPERADMIN) != 0;
    if !election_visible && !census_visible {
        debug!(
            citizen_id = citizen.id,
            "Rejected management page without permissions"
        );
        return themed_error_response(
            StatusCode::FORBIDDEN,
            &ErrorPage::permission("403 Forbidden", "forbidden-page"),
            &state,
            jar,
        )
        .await;
    }
    render_template_page(
        &ManagementPage {
            election_visible,
            census_visible,
        },
        "Management",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

#[derive(Template)]
#[template(path = "management/management.html")]
struct ManagementPage {
    election_visible: bool,
    census_visible: bool,
}
