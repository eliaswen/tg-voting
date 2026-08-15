use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
};
use axum_extra::extract::cookie::CookieJar;
use tracing::{debug, trace};

use crate::pages::auth::{CENSUS_MINISTER, ELECTION_MINISTER, SUPERADMIN, require_citizen};
use crate::pages::login::AppState;
use crate::render::render_page;

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
        return (
            StatusCode::FORBIDDEN,
            Html(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/errors/forbidden.html"
            ))),
        )
            .into_response();
    }
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/management/management.html"
    ))
    .replace(
        "$${{election_management_hidden}}",
        if election_visible { "" } else { "hidden" },
    )
    .replace(
        "$${{census_management_hidden}}",
        if census_visible { "" } else { "hidden" },
    );
    render_page(&content, "Management", jar, &state.pool)
        .await
        .into_response()
}
