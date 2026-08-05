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

use crate::pages::auth::{
    AuthenticatedCitizen, ELECTION_MINISTER, SUPERADMIN, html_escape, require_citizen,
};
use crate::pages::login::AppState;

#[derive(Deserialize)]
pub struct CandidateForm {
    position: String,
    election_display_name: String,
    party: String,
    #[serde(default)]
    vice_president_citizen_id: String,
    #[serde(default)]
    vice_president_display_name: String,
    #[serde(default)]
    vice_president_party: String,
    #[serde(default)]
    message_1: String,
    #[serde(default)]
    message_2: String,
    #[serde(default)]
    message_3: String,
    #[serde(default)]
    message_4: String,
    #[serde(default)]
    message_5: String,
}

pub async fn get_candidate_registration(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let election = match election_registration_state(&state, election_uuid).await {
        Ok(Some(election)) => election,
        Ok(None) => return not_found(),
        Err(response) => return response,
    };
    let election_id: i64 = election.get("id");
    let registration_open: bool = election.get("registration_open");

    let candidate = match sqlx::query(
        "SELECT id, position::text AS position, election_display_name, party, status::text AS status
         FROM candidates WHERE election_id = $1 AND citizen_id = $2 AND status = 'active'",
    )
    .bind(election_id)
    .bind(citizen.id)
    .fetch_optional(&state.pool)
    .await {
        Ok(candidate) => candidate,
        Err(error) => return database_error(error),
    };

    let body = match candidate {
        None if registration_open => {
            let options = match vp_options(&state, election_id, citizen.id, None).await {
                Ok(options) => options,
                Err(response) => return response,
            };
            registration_form(
                election_uuid,
                None,
                &citizen.display_name,
                "",
                &options,
                "",
                "",
                &[
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ],
            )
        }
        None => "<p>Candidate registration is not currently open.</p>".to_string(),
        Some(candidate) => {
            let position: String = candidate.get("position");
            let status: String = candidate.get("status");
            if position == "vice_president" {
                let ticket = match sqlx::query(
                    "SELECT presidential_tickets.status::text AS status,
                            president.election_display_name AS president_name,
                            vice_president.election_display_name AS vice_president_name
                     FROM presidential_tickets
                     JOIN candidates president ON president.id = presidential_tickets.president_candidate_id
                     JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
                     WHERE presidential_tickets.election_id = $1 AND vice_president.id = $2",
                )
                .bind(election_id)
                .bind(candidate.get::<i64, _>("id"))
                .fetch_optional(&state.pool)
                .await {
                    Ok(Some(ticket)) => ticket,
                    Ok(None) => return not_found(),
                    Err(error) => return database_error(error),
                };
                format!(
                    "<p>You are registered as {}'s vice president under the name {}. Ticket status: {}.</p>{}",
                    html_escape(ticket.get("president_name")),
                    html_escape(ticket.get("vice_president_name")),
                    html_escape(ticket.get("status")),
                    withdrawal_form(election_uuid),
                )
            } else if status != "active" {
                format!(
                    "<p>Your {} registration is {} and can no longer be edited.</p>",
                    html_escape(&position),
                    html_escape(&status),
                )
            } else if position == "council" && registration_open {
                registration_form(
                    election_uuid,
                    Some("council"),
                    candidate.get("election_display_name"),
                    candidate.get("party"),
                    "",
                    "",
                    "",
                    &[
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ],
                ) + &withdrawal_form(election_uuid)
            } else if position == "president" {
                let ticket = match sqlx::query(
                    "SELECT presidential_tickets.id, presidential_tickets.status::text AS status,
                            vice_president.citizen_id AS vice_president_citizen_id,
                            vice_president.election_display_name AS vice_president_name,
                            vice_president.party AS vice_president_party
                     FROM presidential_tickets
                     JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
                     WHERE presidential_tickets.election_id = $1 AND presidential_tickets.president_candidate_id = $2",
                )
                .bind(election_id)
                .bind(candidate.get::<i64, _>("id"))
                .fetch_one(&state.pool)
                .await {
                    Ok(ticket) => ticket,
                    Err(error) => return database_error(error),
                };
                let ticket_status: String = ticket.get("status");
                if ticket_status != "active" {
                    format!(
                        "<p>Your presidential ticket is {} and can no longer be edited.</p>",
                        html_escape(&ticket_status),
                    )
                } else if registration_open {
                    let current_vp: i64 = ticket.get("vice_president_citizen_id");
                    let options =
                        match vp_options(&state, election_id, citizen.id, Some(current_vp)).await {
                            Ok(options) => options,
                            Err(response) => return response,
                        };
                    let messages = match load_messages(&state, ticket.get("id")).await {
                        Ok(messages) => messages,
                        Err(response) => return response,
                    };
                    registration_form(
                        election_uuid,
                        Some("president"),
                        candidate.get("election_display_name"),
                        candidate.get("party"),
                        &options,
                        ticket.get("vice_president_name"),
                        ticket.get("vice_president_party"),
                        &messages,
                    ) + &withdrawal_form(election_uuid)
                } else {
                    format!(
                        "<p>Your presidential ticket is active, but candidate registration is closed.</p>{}",
                        withdrawal_form(election_uuid),
                    )
                }
            } else {
                format!(
                    "<p>Your {} registration is active, but candidate registration is closed.</p>{}",
                    html_escape(&position),
                    withdrawal_form(election_uuid),
                )
            }
        }
    };

    let management_link = if citizen.role & (ELECTION_MINISTER | SUPERADMIN) != 0 {
        format!(
            "<p><a href=\"/manage/elections/{}/status\">Manage election status and candidates</a></p>",
            election_uuid,
        )
    } else {
        String::new()
    };

    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Candidate registration</title></head><body>
        <h1>{} candidate registration</h1>{}<p><a href=\"/elections/{}/candidates\">View candidates</a></p>{}</body></html>",
        html_escape(election.get("name")),
        body,
        election_uuid,
        management_link,
    ))
    .into_response()
}

pub async fn post_candidate_registration(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<CandidateForm>,
) -> Response {
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    if let Err(message) = validate_candidate_form(&form) {
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return database_error(error),
    };
    let election = sqlx::query(
        "SELECT id, status::text AS status,
                status = 'registration' AS registration_open
         FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let election = match election {
        Ok(Some(election)) if election.get::<bool, _>("registration_open") => election,
        Ok(Some(_)) => return bad_request("Candidate registration is not currently open."),
        Ok(None) => return not_found(),
        Err(error) => return database_error(error),
    };
    let election_id: i64 = election.get("id");
    let existing = sqlx::query(
        "SELECT id, position::text AS position, status::text AS status
         FROM candidates WHERE election_id = $1 AND citizen_id = $2 AND status = 'active' FOR UPDATE",
    )
    .bind(election_id)
    .bind(citizen.id)
    .fetch_optional(&mut *transaction)
    .await;
    let existing = match existing {
        Ok(existing) => existing,
        Err(error) => return database_error(error),
    };

    let result = match existing {
        Some(existing) if existing.get::<String, _>("status") != "active" => {
            return bad_request("A withdrawn or invalidated registration cannot be edited.");
        }
        Some(existing) if existing.get::<String, _>("position") != form.position => {
            return bad_request("A candidate cannot change roles after registering.");
        }
        Some(existing) if form.position == "council" => {
            sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE id = $3")
                .bind(form.election_display_name.trim())
                .bind(form.party.trim())
                .bind(existing.get::<i64, _>("id"))
                .execute(&mut *transaction)
                .await
                .map(|_| ())
        }
        Some(existing) if form.position == "president" => {
            update_presidential_registration(
                &mut transaction,
                election_id,
                existing.get("id"),
                citizen.id,
                &form,
            )
            .await
        }
        Some(_) => return bad_request("Vice presidents cannot edit the presidential form."),
        None if form.position == "council" => {
            sqlx::query(
                "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
                 VALUES ($1, $2, 'council', $3, $4)",
            )
            .bind(election_id)
            .bind(citizen.id)
            .bind(form.election_display_name.trim())
            .bind(form.party.trim())
            .execute(&mut *transaction)
            .await
            .map(|_| ())
        }
        None => create_presidential_registration(
            &mut transaction,
            election_id,
            citizen.id,
            &form,
        )
        .await,
    };
    if let Err(error) = result {
        return candidate_database_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return database_error(error);
    }
    Redirect::to(&format!("/elections/{election_uuid}/register")).into_response()
}

pub async fn post_withdraw_candidate(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return database_error(error),
    };
    let candidate = sqlx::query(
        "SELECT candidates.id, candidates.uuid, candidates.position::text AS position,
                candidates.status::text AS status, elections.id AS election_id
         FROM candidates JOIN elections ON elections.id = candidates.election_id
         WHERE elections.uuid = $1 AND candidates.citizen_id = $2 AND candidates.status = 'active'
         FOR UPDATE OF candidates",
    )
    .bind(election_uuid)
    .bind(citizen.id)
    .fetch_optional(&mut *transaction)
    .await;
    let candidate = match candidate {
        Ok(Some(candidate)) => candidate,
        Ok(None) => return not_found(),
        Err(error) => return database_error(error),
    };
    let position: String = candidate.get("position");
    let (target_type, target_uuid, previous) = if position == "council" {
        let previous: String = candidate.get("status");
        if previous == "withdrawn" {
            return bad_request("This registration is already withdrawn.");
        }
        if let Err(error) = sqlx::query("UPDATE candidates SET status = 'withdrawn' WHERE id = $1")
            .bind(candidate.get::<i64, _>("id"))
            .execute(&mut *transaction)
            .await
        {
            return database_error(error);
        }
        ("council candidate", candidate.get("uuid"), previous)
    } else {
        let ticket = sqlx::query(
            "SELECT uuid, status::text AS status FROM presidential_tickets
             WHERE president_candidate_id = $1 OR vice_president_candidate_id = $1 FOR UPDATE",
        )
        .bind(candidate.get::<i64, _>("id"))
        .fetch_optional(&mut *transaction)
        .await;
        let ticket = match ticket {
            Ok(Some(ticket)) => ticket,
            Ok(None) => return not_found(),
            Err(error) => return database_error(error),
        };
        let previous: String = ticket.get("status");
        if previous == "withdrawn" {
            return bad_request("This ticket is already withdrawn.");
        }
        if let Err(error) =
            sqlx::query("UPDATE presidential_tickets SET status = 'withdrawn' WHERE uuid = $1")
                .bind(ticket.get::<uuid::Uuid, _>("uuid"))
                .execute(&mut *transaction)
                .await
        {
            return database_error(error);
        }
        if let Err(error) = sqlx::query(
            "UPDATE candidates SET status = 'withdrawn'
             WHERE id IN (
                 SELECT president_candidate_id FROM presidential_tickets WHERE uuid = $1
                 UNION ALL
                 SELECT vice_president_candidate_id FROM presidential_tickets WHERE uuid = $1
             )",
        )
        .bind(ticket.get::<uuid::Uuid, _>("uuid"))
        .execute(&mut *transaction)
        .await
        {
            return database_error(error);
        }
        ("presidential ticket", ticket.get("uuid"), previous)
    };
    if let Err(error) = insert_withdrawal_change(
        &mut transaction,
        candidate.get("election_id"),
        &citizen,
        target_type,
        target_uuid,
        &previous,
    )
    .await
    {
        return database_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return database_error(error);
    }
    Redirect::to(&format!("/elections/{election_uuid}/register")).into_response()
}

pub async fn get_election_candidates(
    State(state): State<AppState>,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    let election = match sqlx::query("SELECT id, name FROM elections WHERE uuid = $1")
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(election)) => election,
        Ok(None) => return not_found(),
        Err(error) => return database_error(error),
    };
    let election_id: i64 = election.get("id");
    let tickets = match sqlx::query(
        "SELECT presidential_tickets.id,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.election_display_name AS vice_president_name, vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN candidates president ON president.id = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1 AND presidential_tickets.status = 'active'
         AND president.status = 'active' AND vice_president.status = 'active'
         ORDER BY president.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await {
        Ok(tickets) => tickets,
        Err(error) => return database_error(error),
    };
    let mut presidential = String::new();
    for ticket in tickets {
        let messages = match load_messages(&state, ticket.get("id")).await {
            Ok(messages) => messages,
            Err(response) => return response,
        };
        let messages = messages
            .iter()
            .filter(|message| !message.is_empty())
            .map(|message| format!("<li>{}</li>", html_escape(message)))
            .collect::<String>();
        presidential.push_str(&format!(
            "<li>{} ({}) with {} ({}){}</li>",
            html_escape(ticket.get("president_name")),
            html_escape(ticket.get("president_party")),
            html_escape(ticket.get("vice_president_name")),
            html_escape(ticket.get("vice_president_party")),
            if messages.is_empty() {
                String::new()
            } else {
                format!("<ul>{messages}</ul>")
            },
        ));
    }
    if presidential.is_empty() {
        presidential.push_str("<li>No presidential tickets are currently registered.</li>");
    }
    let council = match sqlx::query(
        "SELECT election_display_name, party FROM candidates
         WHERE election_id = $1 AND position = 'council' AND status = 'active'
         ORDER BY election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(council) => council,
        Err(error) => return database_error(error),
    };
    let mut council_items = String::new();
    for candidate in council {
        council_items.push_str(&format!(
            "<li>{} ({})</li>",
            html_escape(candidate.get("election_display_name")),
            html_escape(candidate.get("party")),
        ));
    }
    if council_items.is_empty() {
        council_items.push_str("<li>No council candidates are currently registered.</li>");
    }
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Candidates</title></head><body>
        <h1>{} candidates</h1><h2>President</h2><ul>{}</ul><h2>Council</h2><ul>{}</ul>
        <p><a href=\"/elections/{}/register\">Register or manage my candidacy</a></p>
        <p><a href=\"/elections/{}/changes\">Election change log</a></p></body></html>",
        html_escape(election.get("name")),
        presidential,
        council_items,
        election_uuid,
        election_uuid,
    ))
    .into_response()
}

async fn election_registration_state(
    state: &AppState,
    election_uuid: uuid::Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, Response> {
    sqlx::query(
        "SELECT id, name, status::text AS status,
                status = 'registration' AS registration_open
         FROM elections WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)
}

async fn vp_options(
    state: &AppState,
    election_id: i64,
    president_citizen_id: i64,
    current_vp: Option<i64>,
) -> Result<String, Response> {
    let citizens = sqlx::query(
        "SELECT citizens.id,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.id::text) AS display_name
         FROM citizens JOIN authentik_identities ON authentik_identities.citizen_id = citizens.id
         WHERE citizens.banned = FALSE AND citizens.id <> $2
         AND (citizens.id = $3 OR NOT EXISTS (
             SELECT 1 FROM candidates
             WHERE candidates.election_id = $1 AND candidates.citizen_id = citizens.id
             AND candidates.status = 'active'
         )) ORDER BY display_name",
    )
    .bind(election_id)
    .bind(president_citizen_id)
    .bind(current_vp.unwrap_or(-1))
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    let mut options = String::from("<option value=\"\">Select a vice president</option>");
    for citizen in citizens {
        let id: i64 = citizen.get("id");
        options.push_str(&format!(
            "<option value=\"{}\"{}>{}</option>",
            id,
            if Some(id) == current_vp {
                " selected"
            } else {
                ""
            },
            html_escape(citizen.get("display_name")),
        ));
    }
    Ok(options)
}

fn registration_form(
    election_uuid: uuid::Uuid,
    fixed_position: Option<&str>,
    display_name: &str,
    party: &str,
    vp_options: &str,
    vp_display_name: &str,
    vp_party: &str,
    messages: &[String],
) -> String {
    let position = match fixed_position {
        Some(position) => format!("<input type=\"hidden\" name=\"position\" value=\"{}\"><p>Position: {}</p>", position, position),
        None => "<p><label>Position <select name=\"position\"><option value=\"president\">President</option><option value=\"council\">Council</option></select></label></p>".to_string(),
    };
    let message_fields = messages.iter().enumerate().map(|(index, message)| format!(
        "<p><label>Message {} <input type=\"text\" name=\"message_{}\" maxlength=\"100\" value=\"{}\"></label></p>",
        index + 1, index + 1, html_escape(message),
    )).collect::<String>();
    format!(
        "<form method=\"post\" action=\"/elections/{}/register\">{}
        <p><label>Your election display name <input type=\"text\" name=\"election_display_name\" value=\"{}\" required></label></p>
        <p><label>Your party <input type=\"text\" name=\"party\" value=\"{}\" required></label></p>
        <fieldset><legend>Presidential ticket fields</legend>
        <p>These fields are only used when registering for president.</p>
        <p><label>Vice president <select name=\"vice_president_citizen_id\">{}</select></label></p>
        <p><label>Vice president election display name <input type=\"text\" name=\"vice_president_display_name\" value=\"{}\"></label></p>
        <p><label>Vice president party <input type=\"text\" name=\"vice_president_party\" value=\"{}\"></label></p>{}</fieldset>
        <button type=\"submit\">Save registration</button></form>",
        election_uuid,
        position,
        html_escape(display_name),
        html_escape(party),
        vp_options,
        html_escape(vp_display_name),
        html_escape(vp_party),
        message_fields,
    )
}

fn withdrawal_form(election_uuid: uuid::Uuid) -> String {
    format!(
        "<form method=\"post\" action=\"/elections/{}/withdraw\"><button type=\"submit\">Withdraw candidacy</button></form>",
        election_uuid,
    )
}

fn validate_candidate_form(form: &CandidateForm) -> Result<(), &'static str> {
    if !matches!(form.position.as_str(), "president" | "council") {
        return Err("A valid position is required.");
    }
    if form.election_display_name.trim().is_empty() || form.party.trim().is_empty() {
        return Err("Display name and party are required.");
    }
    if form.position == "president" {
        if form.vice_president_citizen_id.parse::<i64>().is_err()
            || form.vice_president_display_name.trim().is_empty()
            || form.vice_president_party.trim().is_empty()
        {
            return Err("A vice president, their display name, and their party are required.");
        }
        validate_messages(&form_messages(form))?;
    }
    Ok(())
}

fn form_messages(form: &CandidateForm) -> Vec<String> {
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

async fn create_presidential_registration(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: i64,
    president_citizen_id: i64,
    form: &CandidateForm,
) -> Result<(), sqlx::Error> {
    let vp_citizen_id = form.vice_president_citizen_id.parse::<i64>().unwrap();
    ensure_vp_available(
        transaction,
        election_id,
        president_citizen_id,
        vp_citizen_id,
        None,
    )
    .await?;
    let president_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
         VALUES ($1, $2, 'president', $3, $4) RETURNING id",
    )
    .bind(election_id)
    .bind(president_citizen_id)
    .bind(form.election_display_name.trim())
    .bind(form.party.trim())
    .fetch_one(&mut **transaction)
    .await?;
    let vp_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
         VALUES ($1, $2, 'vice_president', $3, $4) RETURNING id",
    )
    .bind(election_id)
    .bind(vp_citizen_id)
    .bind(form.vice_president_display_name.trim())
    .bind(form.vice_president_party.trim())
    .fetch_one(&mut **transaction)
    .await?;
    let ticket_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO presidential_tickets (election_id, president_candidate_id, vice_president_candidate_id)
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(election_id)
    .bind(president_id)
    .bind(vp_id)
    .fetch_one(&mut **transaction)
    .await?;
    replace_messages(transaction, ticket_id, &form_messages(form)).await
}

async fn update_presidential_registration(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: i64,
    president_id: i64,
    president_citizen_id: i64,
    form: &CandidateForm,
) -> Result<(), sqlx::Error> {
    let ticket = sqlx::query(
        "SELECT presidential_tickets.id, presidential_tickets.status::text AS status,
                vice_president_candidate_id, vice_president.citizen_id AS old_vp_citizen_id
         FROM presidential_tickets
         JOIN candidates vice_president ON vice_president.id = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1 AND presidential_tickets.president_candidate_id = $2
         FOR UPDATE OF presidential_tickets, vice_president",
    )
    .bind(election_id)
    .bind(president_id)
    .fetch_one(&mut **transaction)
    .await?;
    if ticket.get::<String, _>("status") != "active" {
        return Err(sqlx::Error::Protocol(
            "A withdrawn or invalidated ticket cannot be edited.".to_string(),
        ));
    }
    let vp_citizen_id = form.vice_president_citizen_id.parse::<i64>().unwrap();
    let old_vp_citizen_id: i64 = ticket.get("old_vp_citizen_id");
    ensure_vp_available(
        transaction,
        election_id,
        president_citizen_id,
        vp_citizen_id,
        Some(old_vp_citizen_id),
    )
    .await?;
    let mut vp_id: i64 = ticket.get("vice_president_candidate_id");
    if vp_citizen_id != old_vp_citizen_id {
        let new_vp_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
             VALUES ($1, $2, 'vice_president', $3, $4) RETURNING id",
        )
        .bind(election_id)
        .bind(vp_citizen_id)
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query(
            "UPDATE presidential_tickets SET vice_president_candidate_id = $1 WHERE id = $2",
        )
        .bind(new_vp_id)
        .bind(ticket.get::<i64, _>("id"))
        .execute(&mut **transaction)
        .await?;
        sqlx::query("UPDATE candidates SET status = 'invalidated' WHERE id = $1")
            .bind(vp_id)
            .execute(&mut **transaction)
            .await?;
        vp_id = new_vp_id;
    }
    sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE id = $3")
        .bind(form.election_display_name.trim())
        .bind(form.party.trim())
        .bind(president_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE id = $3")
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .bind(vp_id)
        .execute(&mut **transaction)
        .await?;
    replace_messages(transaction, ticket.get("id"), &form_messages(form)).await
}

async fn ensure_vp_available(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: i64,
    president_citizen_id: i64,
    vp_citizen_id: i64,
    current_vp: Option<i64>,
) -> Result<(), sqlx::Error> {
    if president_citizen_id == vp_citizen_id {
        return Err(sqlx::Error::Protocol(
            "The president cannot also be the vice president.".to_string(),
        ));
    }
    let available = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM citizens JOIN authentik_identities ON authentik_identities.citizen_id = citizens.id
            WHERE citizens.id = $1 AND citizens.banned = FALSE
            AND ($1 = $3 OR NOT EXISTS (
                SELECT 1 FROM candidates
                WHERE candidates.election_id = $2 AND candidates.citizen_id = $1
                AND candidates.status = 'active'
            ))
         )",
    )
    .bind(vp_citizen_id)
    .bind(election_id)
    .bind(current_vp.unwrap_or(-1))
    .fetch_one(&mut **transaction)
    .await?;
    if !available {
        return Err(sqlx::Error::Protocol(
            "That vice president is no longer available.".to_string(),
        ));
    }
    Ok(())
}

async fn load_messages(state: &AppState, ticket_id: i64) -> Result<Vec<String>, Response> {
    let rows = sqlx::query("SELECT position, message FROM presidential_ticket_messages WHERE presidential_ticket_id = $1 ORDER BY position")
        .bind(ticket_id)
        .fetch_all(&state.pool)
        .await
        .map_err(database_error)?;
    let mut messages = vec![String::new(); 5];
    for row in rows {
        messages[(row.get::<i32, _>("position") - 1) as usize] = row.get("message");
    }
    Ok(messages)
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
        sqlx::query("INSERT INTO presidential_ticket_messages (presidential_ticket_id, position, message) VALUES ($1, $2, $3)")
            .bind(ticket_id)
            .bind((index + 1) as i32)
            .bind(message)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn insert_withdrawal_change(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: i64,
    actor: &AuthenticatedCitizen,
    target_type: &str,
    target_uuid: uuid::Uuid,
    previous: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO election_change_log (
            election_id, actor_citizen_id, actor_display_name, target_type, target_uuid,
            previous_value, new_value, reason
         ) VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn', 'Candidate withdrew')",
    )
    .bind(election_id)
    .bind(actor.id)
    .bind(&actor.display_name)
    .bind(target_type)
    .bind(target_uuid)
    .bind(previous)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn candidate_database_error(error: sqlx::Error) -> Response {
    match &error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            bad_request("That user already has a role in this election.")
        }
        sqlx::Error::Protocol(message) => bad_request(message),
        _ => database_error(error),
    }
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html(format!(
            "<h1>Invalid registration</h1><p>{}</p>",
            html_escape(message)
        )),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html("<h1>Election or registration not found</h1>"),
    )
        .into_response()
}

fn database_error(error: sqlx::Error) -> Response {
    error!(?error, "Failed to manage candidate registration");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("<h1>Could not manage candidate registration</h1>"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(position: &str) -> CandidateForm {
        CandidateForm {
            position: position.to_string(),
            election_display_name: "Name".to_string(),
            party: "Party".to_string(),
            vice_president_citizen_id: "2".to_string(),
            vice_president_display_name: "VP".to_string(),
            vice_president_party: "VP Party".to_string(),
            message_1: String::new(),
            message_2: String::new(),
            message_3: String::new(),
            message_4: String::new(),
            message_5: String::new(),
        }
    }

    #[test]
    fn candidate_fields_are_required() {
        assert!(validate_candidate_form(&form("council")).is_ok());
        let mut invalid = form("council");
        invalid.party = " ".to_string();
        assert!(validate_candidate_form(&invalid).is_err());
    }

    #[test]
    fn presidential_fields_are_required() {
        assert!(validate_candidate_form(&form("president")).is_ok());
        let mut invalid = form("president");
        invalid.vice_president_citizen_id = String::new();
        assert!(validate_candidate_form(&invalid).is_err());
    }

    #[test]
    fn messages_are_limited_to_one_hundred_characters() {
        let mut invalid = form("president");
        invalid.message_1 = "a".repeat(101);
        assert!(validate_candidate_form(&invalid).is_err());
    }
}
