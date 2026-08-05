use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use tracing::error;

use crate::pages::auth::{AuthenticatedCitizen, html_escape, require_election_manager};
use crate::pages::login::AppState;

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
        Ok(tickets) => tickets,
        Err(error) => {
            error!(?error, "Failed to retrieve presidential tickets");
            return server_error();
        }
    };

    let mut presidential_forms = String::new();
    for ticket in tickets {
        let ticket_uuid: uuid::Uuid = ticket.get("uuid");
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
        presidential_forms.push_str(&format!(
            "<h3>{} and {}</h3>
            <form method=\"post\" action=\"/manage/elections/{}/status/tickets/{}\">
                <p><label>President display name <input type=\"text\" name=\"president_display_name\" value=\"{}\" required></label></p>
                <p><label>President party <input type=\"text\" name=\"president_party\" value=\"{}\" required></label></p>
                <p><label>Vice president <select name=\"vice_president_citizen_id\">{}</select></label></p>
                <p><label>Vice president display name <input type=\"text\" name=\"vice_president_display_name\" value=\"{}\" required></label></p>
                <p><label>Vice president party <input type=\"text\" name=\"vice_president_party\" value=\"{}\" required></label></p>
                {}
                <p><label>Registration status <select name=\"status\">{}</select></label></p>
                <p><label>Reason <input type=\"text\" name=\"reason\" required></label></p>
                <button type=\"submit\">Update ticket</button>
            </form>",
            html_escape(ticket.get("president_name")),
            html_escape(ticket.get("vice_president_name")),
            election_uuid,
            ticket_uuid,
            html_escape(ticket.get("president_name")),
            html_escape(ticket.get("president_party")),
            vice_president_options,
            html_escape(ticket.get("vice_president_name")),
            html_escape(ticket.get("vice_president_party")),
            message_fields(&messages),
            registration_status_options(ticket.get("status")),
        ));
    }
    if presidential_forms.is_empty() {
        presidential_forms.push_str("<p>No presidential tickets are registered.</p>");
    }

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
        Ok(council) => council,
        Err(error) => {
            error!(?error, "Failed to retrieve council candidates");
            return server_error();
        }
    };

    let mut council_forms = String::new();
    for candidate in council {
        let candidate_uuid: uuid::Uuid = candidate.get("uuid");
        council_forms.push_str(&format!(
            "<h3>{}</h3>
            <form method=\"post\" action=\"/manage/elections/{}/status/council/{}\">
                <p><label>Display name <input type=\"text\" name=\"election_display_name\" value=\"{}\" required></label></p>
                <p><label>Party <input type=\"text\" name=\"party\" value=\"{}\" required></label></p>
                <p><label>Registration status <select name=\"status\">{}</select></label></p>
                <p><label>Reason <input type=\"text\" name=\"reason\" required></label></p>
                <button type=\"submit\">Update candidate</button>
            </form>",
            html_escape(candidate.get("election_display_name")),
            election_uuid,
            candidate_uuid,
            html_escape(candidate.get("election_display_name")),
            html_escape(candidate.get("party")),
            registration_status_options(candidate.get("status")),
        ));
    }
    if council_forms.is_empty() {
        council_forms.push_str("<p>No council candidates are registered.</p>");
    }

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
        Ok(candidates) => candidates,
        Err(error) => {
            error!(?error, "Failed to retrieve former vice presidents");
            return server_error();
        }
    };
    let mut former_vice_president_items = String::new();
    for candidate in former_vice_presidents {
        former_vice_president_items.push_str(&format!(
            "<li>{} ({}) - {}</li>",
            html_escape(candidate.get("election_display_name")),
            html_escape(candidate.get("party")),
            html_escape(candidate.get("status")),
        ));
    }
    if former_vice_president_items.is_empty() {
        former_vice_president_items.push_str("<li>No former vice presidents.</li>");
    }

    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Election status</title></head><body>
        <h1>Season {} status</h1>
        <p>Name: {}</p><p>Current status: {}</p>
        <p>Candidate registration: {} to {}</p>
        <p>Voter code registration: {} to {}</p>
        <p>Voting: {} to {}</p>
        <p>Maximum council choices: {}</p>
        <h2>Change status</h2>
        <form method=\"post\" action=\"/manage/elections/{}/status\">
            <p><label>Status <select name=\"status\">{}</select></label></p>
            <p><label>Reason <input type=\"text\" name=\"reason\" required></label></p>
            <button type=\"submit\">Change status</button>
        </form>
        <h2>Presidential tickets</h2>{}
        <h2>Council candidates</h2>{}
        <h2>Former vice presidents</h2><ul>{}</ul>
        <p><a href=\"/elections/{}/changes\">Public change log</a></p>
        <p><a href=\"/manage/elections\">Back to elections</a></p>
        </body></html>",
        election.get::<i32, _>("season"),
        html_escape(election.get("name")),
        html_escape(&status),
        html_escape(election.get("registration_starts_at")),
        html_escape(election.get("registration_ends_at")),
        html_escape(election.get("voter_code_registration_starts_at")),
        html_escape(election.get("voter_code_registration_ends_at")),
        html_escape(election.get("voting_starts_at")),
        html_escape(election.get("voting_ends_at")),
        election.get::<i32, _>("maximum_council_choices"),
        election_uuid,
        status_options,
        presidential_forms,
        council_forms,
        former_vice_president_items,
        election_uuid,
    ))
    .into_response()
}

pub async fn post_election_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<StatusForm>,
) -> Response {
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if !valid_election_status(&form.status) || form.reason.trim().is_empty() {
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
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn post_manage_council_candidate(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, candidate_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<CouncilCandidateForm>,
) -> Response {
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
        Ok(None) => return not_found(),
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
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn post_manage_presidential_ticket(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((election_uuid, ticket_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<PresidentialTicketForm>,
) -> Response {
    let actor = match require_election_manager(&state, &jar).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(message) = validate_presidential_form(&form) {
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
        Ok(None) => return not_found(),
        Err(error) => return transaction_error(error),
    };
    let president_citizen_id: i64 = ticket.get("president_citizen_id");
    if form.vice_president_citizen_id == president_citizen_id {
        return bad_request("The president cannot also be the vice president.");
    }
    let old_vp_citizen_id: i64 = ticket.get("old_vp_citizen_id");
    let new_vp_id = if form.vice_president_citizen_id == old_vp_citizen_id {
        ticket.get::<i64, _>("vice_president_id")
    } else {
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
            Ok(false) => return bad_request("That vice president is no longer available."),
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
    Redirect::to(&format!("/manage/elections/{election_uuid}/status")).into_response()
}

pub async fn get_election_changes(
    State(state): State<AppState>,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let election_name =
        match sqlx::query_scalar::<_, String>("SELECT name FROM elections WHERE uuid = $1")
            .bind(election_uuid)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(name)) => name,
            Ok(None) => return not_found(),
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
        Ok(changes) => changes,
        Err(error) => return transaction_error(error),
    };
    let mut items = String::new();
    for change in changes {
        items.push_str(&format!(
            "<li>{}: {} changed {} from {} to {}. Reason: {}</li>",
            html_escape(change.get("changed_at")),
            html_escape(change.get("actor_display_name")),
            html_escape(change.get("target_type")),
            html_escape(change.get("previous_value")),
            html_escape(change.get("new_value")),
            html_escape(change.get("reason")),
        ));
    }
    if items.is_empty() {
        items.push_str("<li>No changes have been recorded.</li>");
    }
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Election changes</title></head><body><h1>{} changes</h1><ul>{}</ul><p><a href=\"/elections/{}/candidates\">Candidates</a></p></body></html>",
        html_escape(&election_name),
        items,
        election_uuid,
    ))
    .into_response()
}

async fn eligible_vp_options(
    state: &AppState,
    election_id: i64,
    current_vp: i64,
) -> Result<String, Response> {
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
        Ok(citizens) => citizens,
        Err(error) => return Err(transaction_error(error)),
    };
    Ok(citizens
        .iter()
        .map(|citizen| {
            let id: i64 = citizen.get("id");
            let selected = if id == current_vp { " selected" } else { "" };
            format!(
                "<option value=\"{}\"{}>{}</option>",
                id,
                selected,
                html_escape(citizen.get("display_name")),
            )
        })
        .collect())
}

async fn ticket_messages(
    state: &AppState,
    ticket_uuid: uuid::Uuid,
) -> Result<Vec<String>, Response> {
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
    Ok(messages)
}

fn message_fields(messages: &[String]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            format!(
                "<p><label>Message {} <input type=\"text\" name=\"message_{}\" maxlength=\"100\" value=\"{}\"></label></p>",
                index + 1,
                index + 1,
                html_escape(message),
            )
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
        format!(
            "<option value=\"{}\"{}>{}</option>",
            status,
            if *status == current { " selected" } else { "" },
            status,
        )
    })
    .collect()
}

fn registration_status_options(current: &str) -> String {
    ["active", "withdrawn", "invalidated"]
        .iter()
        .map(|status| {
            format!(
                "<option value=\"{}\"{}>{}</option>",
                status,
                if *status == current { " selected" } else { "" },
                status,
            )
        })
        .collect()
}

fn valid_election_status(status: &str) -> bool {
    matches!(
        status,
        "draft" | "registration" | "voting" | "paused" | "closed" | "canceled" | "certified"
    )
}

fn valid_registration_status(status: &str) -> bool {
    matches!(status, "active" | "withdrawn" | "invalidated")
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
    Ok(())
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html(format!(
            "<h1>Invalid change</h1><p>{}</p>",
            html_escape(message)
        )),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html("<h1>Election item not found</h1>"),
    )
        .into_response()
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("<h1>Could not manage election status</h1>"),
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
