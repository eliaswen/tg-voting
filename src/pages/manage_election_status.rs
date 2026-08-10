use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use tracing::{debug, error, info, trace, warn};

use crate::pages::auth::{AuthenticatedCitizen, html_escape, require_election_manager};
use crate::pages::login::AppState;
use crate::render::render_page;

#[derive(Deserialize)]
pub struct StatusForm {
    status: String,
    reason: String,
}

#[derive(Deserialize)]
pub struct CouncilCandidateForm {
    election_display_name: String,
    party: String,
    status: String,
    reason: String,
}

#[derive(Deserialize)]
pub struct PresidentialTicketForm {
    president_display_name: String,
    president_party: String,
    vice_president_citizen_id: i64,
    vice_president_display_name: String,
    vice_president_party: String,
    message_1: String,
    message_2: String,
    message_3: String,
    message_4: String,
    message_5: String,
    status: String,
    reason: String,
}

pub async fn get_manage_election_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%election_uuid, "Handling election status management page request");
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }

    let election = sqlx::query(
        "SELECT id, season, name, status::text AS status,
                COALESCE(to_char(registration_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS registration_starts_at,
                COALESCE(to_char(registration_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS registration_ends_at,
                COALESCE(to_char(voter_code_registration_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voter_code_registration_starts_at,
                COALESCE(to_char(voter_code_registration_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voter_code_registration_ends_at,
                COALESCE(to_char(voting_starts_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voting_starts_at,
                COALESCE(to_char(voting_ends_at, 'YYYY-MM-DD HH24:MI TZ'), 'Not set') AS voting_ends_at,
                maximum_council_choices
         FROM elections WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await;
    let election = match election {
        Ok(Some(election)) => election,
        Ok(None) => return not_found(),
        Err(error) => {
            error!(?error, "Failed to retrieve election status page");
            return server_error();
        }
    };

    let election_id: i64 = election.get("id");
    let status: String = election.get("status");
    debug!(%election_uuid, election_id, %status, "Retrieved election status management context");
    let status_options = status_options(&status);

    let tickets = match sqlx::query(
        "SELECT presidential_tickets.uuid, presidential_tickets.status::text AS status,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.citizen_id AS vice_president_citizen_id,
                vice_president.election_display_name AS vice_president_name,
                vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN candidates president ON president.id = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1
         ORDER BY president.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await {
        Ok(tickets) => {
            debug!(%election_uuid, ticket_count = tickets.len(), "Retrieved presidential tickets for management");
            tickets
        }
        Err(error) => {
            error!(?error, "Failed to retrieve presidential tickets");
            return server_error();
        }
    };

    let mut presidential_forms = String::new();
    for ticket in tickets {
        let ticket_uuid: uuid::Uuid = ticket.get("uuid");
        trace!(%election_uuid, %ticket_uuid, "Rendering presidential ticket management form");
        let current_vp: i64 = ticket.get("vice_president_citizen_id");
        let vice_president_options =
            match eligible_vp_options(&state, election_id, current_vp).await {
                Ok(options) => options,
                Err(response) => return response,
            };
        let messages = match ticket_messages(&state, ticket_uuid).await {
            Ok(messages) => messages,
            Err(response) => return response,
        };
        presidential_forms.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/presidential-form.html"
            ))
            .replace("$${{election_uuid}}", &election_uuid.to_string())
            .replace("$${{ticket_uuid}}", &ticket_uuid.to_string())
            .replace(
                "$${{president_name}}",
                &html_escape(ticket.get("president_name")),
            )
            .replace(
                "$${{president_party}}",
                &html_escape(ticket.get("president_party")),
            )
            .replace("$${{vice_president_options}}", &vice_president_options)
            .replace(
                "$${{vice_president_name}}",
                &html_escape(ticket.get("vice_president_name")),
            )
            .replace(
                "$${{vice_president_party}}",
                &html_escape(ticket.get("vice_president_party")),
            )
            .replace("$${{message_fields}}", &message_fields(&messages))
            .replace(
                "$${{status_options}}",
                &registration_status_options(ticket.get("status")),
            ),
        );
    }
    let presidential_empty_hidden = if presidential_forms.is_empty() {
        ""
    } else {
        "hidden"
    };

    let council = match sqlx::query(
        "SELECT uuid, election_display_name, party, status::text AS status
         FROM candidates
         WHERE election_id = $1 AND position = 'council'
         ORDER BY election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(council) => {
            debug!(%election_uuid, candidate_count = council.len(), "Retrieved council candidates for management");
            council
        }
        Err(error) => {
            error!(?error, "Failed to retrieve council candidates");
            return server_error();
        }
    };

    let mut council_forms = String::new();
    for candidate in council {
        let candidate_uuid: uuid::Uuid = candidate.get("uuid");
        trace!(%election_uuid, %candidate_uuid, "Rendering council candidate management form");
        council_forms.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/council-form.html"
            ))
            .replace("$${{election_uuid}}", &election_uuid.to_string())
            .replace("$${{candidate_uuid}}", &candidate_uuid.to_string())
            .replace(
                "$${{name}}",
                &html_escape(candidate.get("election_display_name")),
            )
            .replace("$${{party}}", &html_escape(candidate.get("party")))
            .replace(
                "$${{status_options}}",
                &registration_status_options(candidate.get("status")),
            ),
        );
    }
    let council_empty_hidden = if council_forms.is_empty() {
        ""
    } else {
        "hidden"
    };

    let former_vice_presidents = match sqlx::query(
        "SELECT candidates.election_display_name, candidates.party, candidates.status::text AS status
         FROM candidates
         WHERE candidates.election_id = $1
         AND candidates.position = 'vice_president'
         AND NOT EXISTS (
             SELECT 1 FROM presidential_tickets
             WHERE presidential_tickets.vice_president_candidate_id = candidates.id
         )
         ORDER BY candidates.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(candidates) => {
            debug!(%election_uuid, candidate_count = candidates.len(), "Retrieved former vice presidents");
            candidates
        }
        Err(error) => {
            error!(?error, "Failed to retrieve former vice presidents");
            return server_error();
        }
    };
    let mut former_vice_president_items = String::new();
    for candidate in former_vice_presidents {
        former_vice_president_items.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/former-vice-president.html"
            ))
            .replace(
                "$${{name}}",
                &html_escape(candidate.get("election_display_name")),
            )
            .replace("$${{party}}", &html_escape(candidate.get("party")))
            .replace("$${{status}}", &html_escape(candidate.get("status"))),
        );
    }
    let former_vice_presidents_empty_hidden = if former_vice_president_items.is_empty() {
        ""
    } else {
        "hidden"
    };

    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-election-status/status.html"
    ))
    .replace(
        "$${{season}}",
        &election.get::<i32, _>("season").to_string(),
    )
    .replace("$${{name}}", &html_escape(election.get("name")))
    .replace("$${{status}}", &html_escape(&status))
    .replace(
        "$${{registration_starts_at}}",
        &html_escape(election.get("registration_starts_at")),
    )
    .replace(
        "$${{registration_ends_at}}",
        &html_escape(election.get("registration_ends_at")),
    )
    .replace(
        "$${{voter_code_registration_starts_at}}",
        &html_escape(election.get("voter_code_registration_starts_at")),
    )
    .replace(
        "$${{voter_code_registration_ends_at}}",
        &html_escape(election.get("voter_code_registration_ends_at")),
    )
    .replace(
        "$${{voting_starts_at}}",
        &html_escape(election.get("voting_starts_at")),
    )
    .replace(
        "$${{voting_ends_at}}",
        &html_escape(election.get("voting_ends_at")),
    )
    .replace(
        "$${{maximum_council_choices}}",
        &election
            .get::<i32, _>("maximum_council_choices")
            .to_string(),
    )
    .replace("$${{election_uuid}}", &election_uuid.to_string())
    .replace("$${{status_options}}", &status_options)
    .replace("$${{presidential_forms}}", &presidential_forms)
    .replace("$${{council_forms}}", &council_forms)
    .replace("$${{former_vice_presidents}}", &former_vice_president_items)
    .replace("$${{presidential_empty_hidden}}", presidential_empty_hidden)
    .replace("$${{council_empty_hidden}}", council_empty_hidden)
    .replace(
        "$${{former_vice_presidents_empty_hidden}}",
        former_vice_presidents_empty_hidden,
    );
    trace!(%election_uuid, "Rendering election status management page");
    render_page(&content, "Election status", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_election_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<StatusForm>,
) -> Response {
    trace!(%election_uuid, requested_status = %form.status, "Handling election status change");
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if !valid_election_status(&form.status) || form.reason.trim().is_empty() {
        warn!(%election_uuid, actor_citizen_id = actor.id, requested_status = %form.status, reason_present = !form.reason.trim().is_empty(), "Rejected invalid election status change");
        return bad_request("A valid status and reason are required.");
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let previous = sqlx::query_scalar::<_, String>(
        "SELECT status::text FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let previous = match previous {
        Ok(Some(previous)) => previous,
        Ok(None) => return not_found(),
        Err(error) => return transaction_error(error),
    };
    debug!(%election_uuid, actor_citizen_id = actor.id, previous_status = %previous, requested_status = %form.status, "Loaded election status change");
    if let Err(error) =
        sqlx::query("UPDATE elections SET status = $1::election_status WHERE uuid = $2")
            .bind(&form.status)
            .bind(election_uuid)
            .execute(&mut *transaction)
            .await
    {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "election status",
        election_uuid,
        &previous,
        &form.status,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, actor_citizen_id = actor.id, previous_status = %previous, new_status = %form.status, "Changed election status");
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn post_manage_council_candidate(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, candidate_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<CouncilCandidateForm>,
) -> Response {
    trace!(%election_uuid, %candidate_uuid, requested_status = %form.status, "Handling council candidate management update");
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(message) = validate_candidate_fields(
        &form.election_display_name,
        &form.party,
        &form.status,
        &form.reason,
    ) {
        warn!(%election_uuid, %candidate_uuid, actor_citizen_id = actor.id, validation_error = message, "Rejected invalid council candidate update");
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let candidate = sqlx::query(
        "SELECT candidates.election_display_name, candidates.party, candidates.status::text AS status
         FROM candidates JOIN elections ON elections.id = candidates.election_id
         WHERE elections.uuid = $1 AND candidates.uuid = $2 AND candidates.position = 'council'
         FOR UPDATE OF candidates",
    )
    .bind(election_uuid)
    .bind(candidate_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let candidate = match candidate {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            debug!(%election_uuid, %candidate_uuid, "Council candidate management target was not found");
            return not_found();
        }
        Err(error) => return transaction_error(error),
    };
    let previous = format!(
        "{}; party {}; status {}",
        candidate.get::<String, _>("election_display_name"),
        candidate.get::<String, _>("party"),
        candidate.get::<String, _>("status"),
    );
    let new_value = format!(
        "{}; party {}; status {}",
        form.election_display_name.trim(),
        form.party.trim(),
        form.status,
    );
    if let Err(error) = sqlx::query(
        "UPDATE candidates SET election_display_name = $1, party = $2, status = $3::registration_status
         WHERE uuid = $4",
    )
    .bind(form.election_display_name.trim())
    .bind(form.party.trim())
    .bind(&form.status)
    .bind(candidate_uuid)
    .execute(&mut *transaction)
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "council candidate",
        candidate_uuid,
        &previous,
        &new_value,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, %candidate_uuid, actor_citizen_id = actor.id, new_status = %form.status, "Updated council candidate");
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn post_manage_presidential_ticket(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, ticket_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<PresidentialTicketForm>,
) -> Response {
    trace!(%election_uuid, %ticket_uuid, requested_status = %form.status, "Handling presidential ticket management update");
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(message) = validate_presidential_form(&form) {
        warn!(%election_uuid, %ticket_uuid, actor_citizen_id = actor.id, validation_error = message, "Rejected invalid presidential ticket update");
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return transaction_error(error),
    };
    let ticket = sqlx::query(
        "SELECT presidential_tickets.id, presidential_tickets.status::text AS status,
                president.id AS president_id, president.citizen_id AS president_citizen_id,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.id AS vice_president_id, vice_president.citizen_id AS old_vp_citizen_id,
                vice_president.election_display_name AS vice_president_name, vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN elections ON elections.id = presidential_tickets.election_id
         JOIN candidates president ON president.id = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
         WHERE elections.uuid = $1 AND presidential_tickets.uuid = $2
         FOR UPDATE OF presidential_tickets, president, vice_president",
    )
    .bind(election_uuid)
    .bind(ticket_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let ticket = match ticket {
        Ok(Some(ticket)) => ticket,
        Ok(None) => {
            debug!(%election_uuid, %ticket_uuid, "Presidential ticket management target was not found");
            return not_found();
        }
        Err(error) => return transaction_error(error),
    };
    let president_citizen_id: i64 = ticket.get("president_citizen_id");
    if form.vice_president_citizen_id == president_citizen_id {
        return bad_request("The president cannot also be the vice president.");
    }
    let old_vp_citizen_id: i64 = ticket.get("old_vp_citizen_id");
    let new_vp_id = if form.vice_president_citizen_id == old_vp_citizen_id {
        trace!(%ticket_uuid, vice_president_citizen_id = old_vp_citizen_id, "Keeping presidential ticket vice president");
        ticket.get::<i64, _>("vice_president_id")
    } else {
        debug!(%ticket_uuid, old_vp_citizen_id, new_vp_citizen_id = form.vice_president_citizen_id, "Replacing managed presidential ticket vice president");
        let available = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM citizens
                JOIN authentik_identities ON authentik_identities.citizen_id = citizens.id
                JOIN elections ON elections.uuid = $1
                WHERE citizens.id = $2 AND citizens.banned = FALSE
                AND NOT EXISTS (
                    SELECT 1 FROM candidates
                    WHERE candidates.election_id = elections.id AND candidates.citizen_id = citizens.id
                    AND candidates.status = 'active'
                )
            )",
        )
        .bind(election_uuid)
        .bind(form.vice_president_citizen_id)
        .fetch_one(&mut *transaction)
        .await;
        match available {
            Ok(true) => {}
            Ok(false) => {
                warn!(%election_uuid, %ticket_uuid, requested_vp_citizen_id = form.vice_president_citizen_id, "Requested managed ticket vice president is unavailable");
                return bad_request("That vice president is no longer available.");
            }
            Err(error) => return transaction_error(error),
        }
        match sqlx::query_scalar::<_, i64>(
            "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
             SELECT id, $1, 'vice_president', $2, $3 FROM elections WHERE uuid = $4
             RETURNING id",
        )
        .bind(form.vice_president_citizen_id)
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .bind(election_uuid)
        .fetch_one(&mut *transaction)
        .await {
            Ok(id) => id,
            Err(error) => return database_candidate_error(error),
        }
    };

    let previous_messages = match sqlx::query_scalar::<_, String>(
        "SELECT message FROM presidential_ticket_messages WHERE presidential_ticket_id = $1 ORDER BY position",
    )
    .bind(ticket.get::<i64, _>("id"))
    .fetch_all(&mut *transaction)
    .await {
        Ok(messages) => messages.join(" | "),
        Err(error) => return transaction_error(error),
    };
    let previous = format!(
        "{} ({}) and {} ({}); messages [{}]; status {}",
        ticket.get::<String, _>("president_name"),
        ticket.get::<String, _>("president_party"),
        ticket.get::<String, _>("vice_president_name"),
        ticket.get::<String, _>("vice_president_party"),
        previous_messages,
        ticket.get::<String, _>("status"),
    );
    let messages = form_messages(&form);
    let new_value = format!(
        "{} ({}) and {} ({}); messages [{}]; status {}",
        form.president_display_name.trim(),
        form.president_party.trim(),
        form.vice_president_display_name.trim(),
        form.vice_president_party.trim(),
        messages.join(" | "),
        form.status,
    );

    if let Err(error) =
        sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE id = $3")
            .bind(form.president_display_name.trim())
            .bind(form.president_party.trim())
            .bind(ticket.get::<i64, _>("president_id"))
            .execute(&mut *transaction)
            .await
    {
        return transaction_error(error);
    }
    if let Err(error) =
        sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE id = $3")
            .bind(form.vice_president_display_name.trim())
            .bind(form.vice_president_party.trim())
            .bind(new_vp_id)
            .execute(&mut *transaction)
            .await
    {
        return transaction_error(error);
    }
    if let Err(error) = sqlx::query(
        "UPDATE presidential_tickets SET vice_president_candidate_id = $1, status = $2::registration_status WHERE id = $3",
    )
    .bind(new_vp_id)
    .bind(&form.status)
    .bind(ticket.get::<i64, _>("id"))
    .execute(&mut *transaction)
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = sqlx::query(
        "UPDATE candidates SET status = $1::registration_status WHERE id = $2 OR id = $3",
    )
    .bind(&form.status)
    .bind(ticket.get::<i64, _>("president_id"))
    .bind(new_vp_id)
    .execute(&mut *transaction)
    .await
    {
        return database_candidate_error(error);
    }
    if new_vp_id != ticket.get::<i64, _>("vice_president_id") {
        if let Err(error) =
            sqlx::query("UPDATE candidates SET status = 'invalidated' WHERE id = $1")
                .bind(ticket.get::<i64, _>("vice_president_id"))
                .execute(&mut *transaction)
                .await
        {
            return transaction_error(error);
        }
    }
    if let Err(error) = replace_messages(&mut transaction, ticket.get("id"), &messages).await {
        return transaction_error(error);
    }
    if let Err(error) = insert_change(
        &mut transaction,
        election_uuid,
        &actor,
        "presidential ticket",
        ticket_uuid,
        &previous,
        &new_value,
        &form.reason,
    )
    .await
    {
        return transaction_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return transaction_error(error);
    }
    info!(%election_uuid, %ticket_uuid, actor_citizen_id = actor.id, new_status = %form.status, vice_president_changed = form.vice_president_citizen_id != old_vp_citizen_id, message_count = messages.len(), "Updated presidential ticket");
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn get_election_changes(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    trace!(%election_uuid, "Handling election change log request");
    let election_name =
        match sqlx::query_scalar::<_, String>("SELECT name FROM elections WHERE uuid = $1")
            .bind(election_uuid)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(name)) => name,
            Ok(None) => {
                debug!(%election_uuid, "Election change log target was not found");
                return not_found();
            }
            Err(error) => return transaction_error(error),
        };
    let changes = match sqlx::query(
        "SELECT actor_display_name, target_type, previous_value, new_value, reason,
                to_char(election_change_log.database_created_at, 'YYYY-MM-DD HH24:MI:SS TZ') AS changed_at
         FROM election_change_log
         JOIN elections ON elections.id = election_change_log.election_id
         WHERE elections.uuid = $1
         ORDER BY election_change_log.database_created_at DESC, election_change_log.id DESC",
    )
    .bind(election_uuid)
    .fetch_all(&state.pool)
    .await
    {
        Ok(changes) => {
            debug!(%election_uuid, change_count = changes.len(), "Retrieved election change log");
            changes
        }
        Err(error) => return transaction_error(error),
    };
    let mut items = String::new();
    for change in changes {
        items.push_str(
            &include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/change-item.html"
            ))
            .replace("$${{changed_at}}", &html_escape(change.get("changed_at")))
            .replace(
                "$${{actor}}",
                &html_escape(change.get("actor_display_name")),
            )
            .replace("$${{target}}", &html_escape(change.get("target_type")))
            .replace("$${{previous}}", &html_escape(change.get("previous_value")))
            .replace("$${{new}}", &html_escape(change.get("new_value")))
            .replace("$${{reason}}", &html_escape(change.get("reason"))),
        );
    }
    let empty_hidden = if items.is_empty() { "" } else { "hidden" };
    let content = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/static/manage-election-status/changes.html"
    ))
    .replace("$${{election_name}}", &html_escape(&election_name))
    .replace("$${{change_items}}", &items)
    .replace("$${{empty_hidden}}", empty_hidden)
    .replace("$${{election_uuid}}", &election_uuid.to_string());
    trace!(%election_uuid, "Rendering election change log page");
    render_page(&content, "Election changes", jar, &state.pool)
        .await
        .into_response()
}

async fn eligible_vp_options(
    state: &AppState,
    election_id: i64,
    current_vp: i64,
) -> Result<String, Response> {
    trace!(
        election_id,
        current_vp, "Retrieving manager vice president options"
    );
    let citizens = sqlx::query(
        "SELECT citizens.id,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.id::text) AS display_name
         FROM citizens
         JOIN authentik_identities ON authentik_identities.citizen_id = citizens.id
         WHERE citizens.banned = FALSE
         AND (citizens.id = $2 OR NOT EXISTS (
             SELECT 1 FROM candidates
             WHERE candidates.election_id = $1 AND candidates.citizen_id = citizens.id
             AND candidates.status = 'active'
         ))
         ORDER BY display_name",
    )
    .bind(election_id)
    .bind(current_vp)
    .fetch_all(&state.pool)
    .await;
    let citizens = match citizens {
        Ok(citizens) => {
            debug!(
                election_id,
                citizen_count = citizens.len(),
                "Retrieved manager vice president options"
            );
            citizens
        }
        Err(error) => return Err(transaction_error(error)),
    };
    Ok(citizens
        .iter()
        .map(|citizen| {
            let id: i64 = citizen.get("id");
            let selected = if id == current_vp { " selected" } else { "" };
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/form-option.html"
            ))
            .replace("$${{value}}", &id.to_string())
            .replace("$${{selected}}", selected.trim())
            .replace("$${{label}}", &html_escape(citizen.get("display_name")))
        })
        .collect())
}

async fn ticket_messages(
    state: &AppState,
    ticket_uuid: uuid::Uuid,
) -> Result<Vec<String>, Response> {
    trace!(%ticket_uuid, "Loading managed presidential ticket messages");
    let rows = sqlx::query(
        "SELECT presidential_ticket_messages.position, presidential_ticket_messages.message
         FROM presidential_ticket_messages
         JOIN presidential_tickets ON presidential_tickets.id = presidential_ticket_messages.presidential_ticket_id
         WHERE presidential_tickets.uuid = $1 ORDER BY presidential_ticket_messages.position",
    )
    .bind(ticket_uuid)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return Err(transaction_error(error)),
    };
    let mut messages = vec![String::new(); 5];
    for row in rows {
        let position: i32 = row.get("position");
        messages[(position - 1) as usize] = row.get("message");
    }
    debug!(%ticket_uuid, message_count = messages.iter().filter(|message| !message.is_empty()).count(), "Loaded managed presidential ticket messages");
    Ok(messages)
}

fn message_fields(messages: &[String]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/candidates/message-field.html"
            ))
            .replace("$${{position}}", &(index + 1).to_string())
            .replace("$${{message}}", &html_escape(message))
        })
        .collect()
}

fn status_options(current: &str) -> String {
    [
        "draft",
        "registration",
        "voting",
        "paused",
        "closed",
        "canceled",
        "certified",
    ]
    .iter()
    .map(|status| {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/manage-election-status/form-option.html"
        ))
        .replace("$${{value}}", status)
        .replace(
            "$${{selected}}",
            if *status == current { "selected" } else { "" },
        )
        .replace("$${{label}}", status)
    })
    .collect()
}

fn registration_status_options(current: &str) -> String {
    ["active", "withdrawn", "invalidated"]
        .iter()
        .map(|status| {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/form-option.html"
            ))
            .replace("$${{value}}", status)
            .replace(
                "$${{selected}}",
                if *status == current { "selected" } else { "" },
            )
            .replace("$${{label}}", status)
        })
        .collect()
}

fn valid_election_status(status: &str) -> bool {
    let valid = matches!(
        status,
        "draft" | "registration" | "voting" | "paused" | "closed" | "canceled" | "certified"
    );
    trace!(status, valid, "Validated election status");
    valid
}

fn valid_registration_status(status: &str) -> bool {
    let valid = matches!(status, "active" | "withdrawn" | "invalidated");
    trace!(status, valid, "Validated registration status");
    valid
}

fn validate_candidate_fields<'a>(
    display_name: &str,
    party: &str,
    status: &str,
    reason: &str,
) -> Result<(), &'a str> {
    if display_name.trim().is_empty() || party.trim().is_empty() {
        return Err("Display name and party are required.");
    }
    if !valid_registration_status(status) {
        return Err("Invalid registration status.");
    }
    if reason.trim().is_empty() {
        return Err("A reason is required.");
    }
    Ok(())
}

fn validate_presidential_form(form: &PresidentialTicketForm) -> Result<(), &'static str> {
    validate_candidate_fields(
        &form.president_display_name,
        &form.president_party,
        &form.status,
        &form.reason,
    )?;
    if form.vice_president_display_name.trim().is_empty()
        || form.vice_president_party.trim().is_empty()
    {
        return Err("Vice president display name and party are required.");
    }
    validate_messages(&form_messages(form))
}

fn form_messages(form: &PresidentialTicketForm) -> Vec<String> {
    [
        &form.message_1,
        &form.message_2,
        &form.message_3,
        &form.message_4,
        &form.message_5,
    ]
    .iter()
    .map(|message| message.trim())
    .filter(|message| !message.is_empty())
    .map(str::to_string)
    .collect()
}

fn validate_messages(messages: &[String]) -> Result<(), &'static str> {
    if messages.len() > 5 || messages.iter().any(|message| message.chars().count() > 100) {
        return Err("Tickets may have up to five messages of 100 characters each.");
    }
    Ok(())
}

async fn replace_messages(
    transaction: &mut Transaction<'_, Postgres>,
    ticket_id: i64,
    messages: &[String],
) -> Result<(), sqlx::Error> {
    trace!(
        ticket_id,
        message_count = messages.len(),
        "Replacing managed ticket messages"
    );
    sqlx::query("DELETE FROM presidential_ticket_messages WHERE presidential_ticket_id = $1")
        .bind(ticket_id)
        .execute(&mut **transaction)
        .await?;
    for (index, message) in messages.iter().enumerate() {
        sqlx::query(
            "INSERT INTO presidential_ticket_messages (presidential_ticket_id, position, message) VALUES ($1, $2, $3)",
        )
        .bind(ticket_id)
        .bind((index + 1) as i32)
        .bind(message)
        .execute(&mut **transaction)
        .await?;
    }
    debug!(
        ticket_id,
        message_count = messages.len(),
        "Replaced managed ticket messages"
    );
    Ok(())
}

async fn insert_change(
    transaction: &mut Transaction<'_, Postgres>,
    election_uuid: uuid::Uuid,
    actor: &AuthenticatedCitizen,
    target_type: &str,
    target_uuid: uuid::Uuid,
    previous_value: &str,
    new_value: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    trace!(%election_uuid, actor_citizen_id = actor.id, target_type, %target_uuid, "Recording election management change");
    sqlx::query(
        "INSERT INTO election_change_log (
            election_id, actor_citizen_id, actor_display_name, target_type, target_uuid,
            previous_value, new_value, reason
         ) SELECT id, $1, $2, $3, $4, $5, $6, $7 FROM elections WHERE uuid = $8",
    )
    .bind(actor.id)
    .bind(&actor.display_name)
    .bind(target_type)
    .bind(target_uuid)
    .bind(previous_value)
    .bind(new_value)
    .bind(reason.trim())
    .bind(election_uuid)
    .execute(&mut **transaction)
    .await?;
    debug!(%election_uuid, actor_citizen_id = actor.id, target_type, %target_uuid, "Recorded election management change");
    Ok(())
}

fn bad_request(message: &str) -> Response {
    debug!(
        validation_error = message,
        "Returning invalid election status response"
    );
    (
        StatusCode::BAD_REQUEST,
        Html(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static/manage-election-status/invalid.html"
            ))
            .replace("$${{message}}", &html_escape(message)),
        ),
    )
        .into_response()
}

fn not_found() -> Response {
    debug!("Returning election status not found response");
    (
        StatusCode::NOT_FOUND,
        Html(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/manage-election-status/not-found.html"
        ))),
    )
        .into_response()
}

fn server_error() -> Response {
    error!("Returning election status server error response");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/static/manage-election-status/error.html"
        ))),
    )
        .into_response()
}

fn transaction_error(error: sqlx::Error) -> Response {
    error!(?error, "Failed to change election status data");
    server_error()
}

fn database_candidate_error(error: sqlx::Error) -> Response {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() {
            warn!("Election status update violated candidate uniqueness");
            return bad_request("That user already has a role in this election.");
        }
    }
    transaction_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_validated() {
        assert!(valid_election_status("paused"));
        assert!(valid_election_status("canceled"));
        assert!(!valid_election_status("unknown"));
    }

    #[test]
    fn messages_are_limited() {
        assert!(validate_messages(&vec!["a".repeat(100); 5]).is_ok());
        assert!(validate_messages(&vec!["a".repeat(101)]).is_err());
        assert!(validate_messages(&vec!["a".to_string(); 6]).is_err());
    }
}
