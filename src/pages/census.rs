use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::Row;
use tracing::{debug, error, info, trace, warn};

use crate::error_handling::{ErrorPage, error_response};
use crate::pages::auth::require_census_manager;
use crate::pages::login::AppState;
use crate::render::render_template_page;

#[derive(Deserialize)]
pub struct CreateCensusForm {
    census_month: String,
}

#[derive(Deserialize)]
pub struct CensusCitizenForm {
    citizen_id: String,
    status: String,
}

#[derive(Deserialize, Default)]
pub struct CensusSearch {
    #[serde(default)]
    q: String,
}

pub async fn get_census(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(search): Query<CensusSearch>,
) -> Response {
    render_census(state, jar, None, search.q).await
}

pub async fn get_census_month(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(census_uuid): Path<uuid::Uuid>,
    Query(search): Query<CensusSearch>,
) -> Response {
    render_census(state, jar, Some(census_uuid), search.q).await
}

async fn render_census(
    state: AppState,
    jar: CookieJar,
    selected_uuid: Option<uuid::Uuid>,
    search: String,
) -> Response {
    trace!(?selected_uuid, "Handling census management page request");
    let manager = match require_census_manager(&state, &jar).await {
        Ok(manager) => manager,
        Err(response) => return response,
    };

    let censuses = match sqlx::query(
        "SELECT uuid, census_month, active, database_created_at, activated_at
         FROM censuses
         ORDER BY census_month DESC",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(censuses) => censuses,
        Err(error) => {
            error!(
                ?error,
                citizen_id = manager.id,
                "Failed to retrieve censuses"
            );
            return census_server_error();
        }
    };

    let selected = selected_uuid.and_then(|uuid| {
        censuses
            .iter()
            .find(|census| census.get::<uuid::Uuid, _>("uuid") == uuid)
    });

    if selected_uuid.is_some() && selected.is_none() {
        warn!(
            citizen_id = manager.id,
            ?selected_uuid,
            "Requested census was not found"
        );
        return error_response(
            StatusCode::NOT_FOUND,
            &ErrorPage::new("404 Not Found", "", "not-found-page").with_message_kind(2),
        );
    }

    let mut census_items = Vec::new();
    for census in &censuses {
        let census_uuid: uuid::Uuid = census.get("uuid");
        let month: NaiveDate = census.get("census_month");
        let active: bool = census.get("active");
        census_items.push(CensusItem {
            uuid: census_uuid,
            month: month.format("%B %Y").to_string(),
            active,
        });
    }

    if selected_uuid.is_none() {
        let active = censuses
            .iter()
            .find(|census| census.get::<bool, _>("active"));
        let (active_uuid, active_month, has_active) = match active {
            Some(census) => {
                let census_uuid: uuid::Uuid = census.get("uuid");
                let month: NaiveDate = census.get("census_month");
                (
                    census_uuid.to_string(),
                    month.format("%B %Y").to_string(),
                    true,
                )
            }
            None => (String::new(), String::new(), false),
        };
        let current_month = chrono::Utc::now().date_naive();
        let current_month = format!("{:04}-{:02}", current_month.year(), current_month.month());
        let page = CensusDashboardPage {
            current_month: &current_month,
            censuses: &census_items,
            active_uuid: &active_uuid,
            active_month: &active_month,
            has_active,
        };
        debug!(citizen_id = manager.id, "Rendering census dashboard");
        return render_template_page(&page, "Manage census", jar, &state.pool)
            .await
            .into_response();
    }

    let (selected_census_uuid, selected_month, selected_active, citizen_rows) =
        if let Some(census) = selected {
            let census_uuid: uuid::Uuid = census.get("uuid");
            let citizens = match sqlx::query(
                "SELECT citizens.uuid, citizens.citizen_id,
                        COALESCE(authentik_identities.preferred_username, '') AS oauth_username,
                        COALESCE(authentik_identities.display_name, '') AS display_name,
                        COALESCE(citizen_discord_links.discord_username, '') AS discord_username,
                        COALESCE(citizen_reddit_links.reddit_username, '') AS reddit_username,
                        COALESCE(census_entries.status::text, 'not_filled_out') AS census_status
                 FROM citizens
                 LEFT JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
                 LEFT JOIN citizen_discord_links ON citizen_discord_links.citizen_id = citizens.uuid
                 LEFT JOIN citizen_reddit_links ON citizen_reddit_links.citizen_id = citizens.uuid
                 LEFT JOIN census_entries
                    ON census_entries.citizen_uuid = citizens.uuid
                    AND census_entries.census_uuid = $1
                 WHERE $2 = '%%'
                    OR COALESCE(authentik_identities.preferred_username, '') ILIKE $2
                    OR COALESCE(authentik_identities.display_name, '') ILIKE $2
                    OR COALESCE(citizen_discord_links.discord_username, '') ILIKE $2
                    OR COALESCE(citizen_reddit_links.reddit_username, '') ILIKE $2
                    OR COALESCE(citizens.citizen_id, '') ILIKE $2
                    OR COALESCE(census_entries.status::text, 'not_filled_out') ILIKE $3
                 ORDER BY lower(COALESCE(authentik_identities.display_name,
                                         authentik_identities.preferred_username,
                                         citizens.citizen_id,
                                         citizens.uuid::text))",
            )
            .bind(census_uuid)
            .bind(format!("%{}%", search.trim()))
            .bind(format!("%{}%", search.trim().replace(' ', "_")))
            .fetch_all(&state.pool)
            .await
            {
                Ok(citizens) => citizens,
                Err(error) => {
                    error!(?error, %census_uuid, "Failed to retrieve census citizens");
                    return census_server_error();
                }
            };
            let mut rows = Vec::new();
            for citizen in citizens {
                let citizen_uuid: uuid::Uuid = citizen.get("uuid");
                let status: String = citizen.get("census_status");
                let citizen_identifier = citizen
                    .try_get::<Option<String>, _>("citizen_id")
                    .unwrap_or_default()
                    .unwrap_or_default();
                rows.push(CensusCitizen {
                    uuid: citizen_uuid,
                    citizen_id: citizen_identifier,
                    oauth_username: display_value_raw(citizen.get("oauth_username")),
                    display_name: display_value_raw(citizen.get("display_name")),
                    discord_username: display_value_raw(citizen.get("discord_username")),
                    reddit_username: display_value_raw(citizen.get("reddit_username")),
                    status,
                });
            }
            let month: NaiveDate = census.get("census_month");
            (
                census_uuid.to_string(),
                month.format("%B %Y").to_string(),
                census.get::<bool, _>("active"),
                rows,
            )
        } else {
            (String::new(), String::new(), false, Vec::new())
        };

    let page = CensusMonthPage {
        search_query: search.trim(),
        census_uuid: &selected_census_uuid,
        selected_month: &selected_month,
        selected_status: if selected_active {
            "Currently used"
        } else {
            "Not currently used"
        },
        active: selected_active,
        citizens: &citizen_rows,
    };
    debug!(
        citizen_id = manager.id,
        census_uuid = %selected_census_uuid,
        "Rendering monthly census page"
    );
    render_template_page(
        &page,
        &format!("{} census", selected_month),
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

pub async fn post_create_census(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<CreateCensusForm>,
) -> Response {
    trace!(census_month = %form.census_month, "Handling census creation request");
    let manager = match require_census_manager(&state, &jar).await {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    let month = match NaiveDate::parse_from_str(&format!("{}-01", form.census_month), "%Y-%m-%d") {
        Ok(month) => month,
        Err(_) => return census_bad_request("The census month is invalid."),
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            error!(?error, "Failed to start census creation transaction");
            return census_server_error();
        }
    };
    let census = sqlx::query(
        "INSERT INTO censuses (census_month, created_by_citizen_uuid)
         VALUES ($1, $2)
         RETURNING uuid",
    )
    .bind(month)
    .bind(manager.uuid)
    .fetch_one(&mut *transaction)
    .await;
    let census = match census {
        Ok(census) => census,
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|error| error.is_unique_violation()) =>
        {
            warn!(%month, citizen_id = manager.id, "Rejected duplicate monthly census");
            return census_bad_request("A census already exists for that month.");
        }
        Err(error) => {
            error!(?error, %month, "Failed to create census");
            return census_server_error();
        }
    };
    let census_uuid: uuid::Uuid = census.get("uuid");
    if let Err(error) = sqlx::query(
        "INSERT INTO census_entries (census_uuid, citizen_uuid)
         SELECT $1, citizens.uuid FROM citizens",
    )
    .bind(census_uuid)
    .execute(&mut *transaction)
    .await
    {
        error!(?error, %census_uuid, "Failed to initialize census entries");
        return census_server_error();
    }
    if let Err(error) = transaction.commit().await {
        error!(?error, %census_uuid, "Failed to commit census creation");
        return census_server_error();
    }
    info!(%census_uuid, %month, citizen_id = manager.id, "Created monthly census");
    Redirect::to(&format!("/manage/census/{census_uuid}")).into_response()
}

pub async fn post_activate_census(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(census_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%census_uuid, "Handling census activation request");
    let manager = match require_census_manager(&state, &jar).await {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            error!(?error, "Failed to start census activation transaction");
            return census_server_error();
        }
    };
    if let Err(error) = sqlx::query("UPDATE censuses SET active = FALSE WHERE active = TRUE")
        .execute(&mut *transaction)
        .await
    {
        error!(?error, %census_uuid, "Failed to deactivate previous census");
        return census_server_error();
    }
    let result = sqlx::query(
        "UPDATE censuses
         SET active = TRUE, activated_at = CURRENT_TIMESTAMP
         WHERE uuid = $1",
    )
    .bind(census_uuid)
    .execute(&mut *transaction)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => return census_not_found(),
        Err(error) => {
            error!(?error, %census_uuid, "Failed to activate census");
            return census_server_error();
        }
    }
    if let Err(error) = transaction.commit().await {
        error!(?error, %census_uuid, "Failed to commit census activation");
        return census_server_error();
    }
    info!(%census_uuid, citizen_id = manager.id, "Activated census");
    Redirect::to(&format!("/manage/census/{census_uuid}")).into_response()
}

pub async fn post_update_census_citizen(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((census_uuid, citizen_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<CensusCitizenForm>,
) -> Response {
    trace!(%census_uuid, %citizen_uuid, status = %form.status, "Handling census citizen update");
    let manager = match require_census_manager(&state, &jar).await {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    if !valid_status(&form.status) {
        return census_bad_request("The selected census status is invalid.");
    }
    let citizen_identifier = match form.citizen_id.trim() {
        "" => None,
        value if value.len() <= 100 => Some(value),
        _ => return census_bad_request("The citizen ID must be 100 characters or fewer."),
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            error!(?error, "Failed to start census citizen transaction");
            return census_server_error();
        }
    };
    let update = sqlx::query("UPDATE citizens SET citizen_id = $1 WHERE uuid = $2 RETURNING uuid")
        .bind(citizen_identifier)
        .bind(citizen_uuid)
        .fetch_optional(&mut *transaction)
        .await;
    let saved_citizen_uuid: uuid::Uuid = match update {
        Ok(Some(row)) => row.get("uuid"),
        Ok(None) => return census_not_found(),
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|error| error.is_unique_violation()) =>
        {
            return census_bad_request("That citizen ID is already assigned to another citizen.");
        }
        Err(error) => {
            error!(?error, %citizen_uuid, "Failed to update citizen ID");
            return census_server_error();
        }
    };
    let result = sqlx::query(
        "INSERT INTO census_entries
            (census_uuid, citizen_uuid, status, last_updated_by_citizen_uuid)
         SELECT censuses.uuid, $2, $3::census_status, $4
         FROM censuses WHERE censuses.uuid = $1
         ON CONFLICT (census_uuid, citizen_uuid) DO UPDATE
         SET status = EXCLUDED.status,
             last_updated_by_citizen_uuid = EXCLUDED.last_updated_by_citizen_uuid",
    )
    .bind(census_uuid)
    .bind(saved_citizen_uuid)
    .bind(&form.status)
    .bind(manager.uuid)
    .execute(&mut *transaction)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => return census_not_found(),
        Err(error) => {
            error!(?error, %census_uuid, %citizen_uuid, "Failed to update census status");
            return census_server_error();
        }
    }
    if let Err(error) = transaction.commit().await {
        error!(?error, %census_uuid, %citizen_uuid, "Failed to commit census citizen update");
        return census_server_error();
    }
    info!(%census_uuid, %citizen_uuid, status = %form.status, citizen_id = manager.id, "Updated census citizen");
    Redirect::to(&format!("/manage/census/{census_uuid}")).into_response()
}

fn display_value_raw(value: String) -> String {
    if value.trim().is_empty() {
        "Not available".to_string()
    } else {
        value
    }
}

struct CensusItem {
    uuid: uuid::Uuid,
    month: String,
    active: bool,
}

#[derive(Template)]
#[template(path = "census/dashboard.html")]
struct CensusDashboardPage<'a> {
    current_month: &'a str,
    censuses: &'a [CensusItem],
    active_uuid: &'a str,
    active_month: &'a str,
    has_active: bool,
}

struct CensusCitizen {
    uuid: uuid::Uuid,
    citizen_id: String,
    oauth_username: String,
    display_name: String,
    discord_username: String,
    reddit_username: String,
    status: String,
}

#[derive(Template)]
#[template(path = "census/month.html")]
struct CensusMonthPage<'a> {
    search_query: &'a str,
    census_uuid: &'a str,
    selected_month: &'a str,
    selected_status: &'a str,
    active: bool,
    citizens: &'a [CensusCitizen],
}

fn valid_status(status: &str) -> bool {
    matches!(
        status,
        "filled_out" | "ineligible" | "incorrect" | "not_filled_out" | "other" | "to_be_set"
    )
}

fn census_bad_request(message: &str) -> Response {
    warn!(%message, "Rejected census request");
    error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new("Invalid census request", message, "census-error-page")
            .with_back("/manage/census", "Return to census management")
            .with_back_period(),
    )
}

fn census_not_found() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &ErrorPage::new("404 Not Found", "", "not-found-page").with_message_kind(2),
    )
}

fn census_server_error() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new("Account Page", "There was an error while trying to retrieve your account information. This error should not be your fault.", "account-error-page").with_message_kind(5),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_statuses_are_validated() {
        assert!(valid_status("filled_out"));
        assert!(valid_status("ineligible"));
        assert!(valid_status("incorrect"));
        assert!(valid_status("not_filled_out"));
        assert!(valid_status("other"));
        assert!(!valid_status("complete"));
    }
}
