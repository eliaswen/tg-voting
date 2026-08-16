use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use tracing::{debug, error, info, trace, warn};

use crate::error_handling::{ErrorPage, error_response};
use crate::pages::auth::{AuthenticatedCitizen, ELECTION_MINISTER, SUPERADMIN, require_citizen};
use crate::pages::login::AppState;
use crate::render::render_template_page;

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
    #[serde(default)]
    debug_census: bool,
}

#[derive(Deserialize, Default)]
pub struct CandidateSearch {
    #[serde(default)]
    q: String,
}

#[derive(Deserialize, Default)]
pub struct RegistrationSearch {
    #[serde(default)]
    position: String,
}

struct CandidateRegistrationPage {
    show_form: bool,
    show_status: bool,
    show_withdrawal: bool,
    status_message: String,
    position: Option<String>,
    display_name: String,
    party: String,
    vice_president_options: Vec<VicePresidentOption>,
    vice_president_name: String,
    vice_president_party: String,
    messages: Vec<String>,
}

impl CandidateRegistrationPage {
    fn new() -> Self {
        Self {
            show_form: false,
            show_status: false,
            show_withdrawal: false,
            status_message: String::new(),
            position: None,
            display_name: String::new(),
            party: String::new(),
            vice_president_options: Vec::new(),
            vice_president_name: String::new(),
            vice_president_party: String::new(),
            messages: vec![String::new(); 5],
        }
    }

    fn show_status(&mut self, message: String, show_withdrawal: bool) {
        self.show_status = true;
        self.show_withdrawal = show_withdrawal;
        self.status_message = message;
    }

    fn show_form(
        &mut self,
        position: Option<&str>,
        display_name: &str,
        party: &str,
        vice_president_options: Vec<VicePresidentOption>,
        vice_president_name: &str,
        vice_president_party: &str,
        messages: Vec<String>,
        show_withdrawal: bool,
    ) {
        self.show_form = true;
        self.show_withdrawal = show_withdrawal;
        self.position = position.map(str::to_string);
        self.display_name = display_name.to_string();
        self.party = party.to_string();
        self.vice_president_options = vice_president_options;
        self.vice_president_name = vice_president_name.to_string();
        self.vice_president_party = vice_president_party.to_string();
        self.messages = messages;
    }
}

struct VicePresidentOption {
    id: uuid::Uuid,
    selected: bool,
    display_name: String,
}

#[derive(Template)]
#[template(path = "candidates/registration.html")]
struct CandidateRegistrationTemplate<'a> {
    election_name: String,
    election_uuid: uuid::Uuid,
    position: &'a str,
    page: &'a CandidateRegistrationPage,
    management_visible: bool,
    positions: &'a [RegistrationPosition],
    presidential: bool,
    debug_mode: bool,
}

struct RegistrationPosition {
    value: String,
    label: String,
    selected: bool,
}

struct PresidentialTicket {
    president_name: String,
    president_party: String,
    vice_president_name: String,
    vice_president_party: String,
    messages: Vec<String>,
}

struct CouncilCandidate {
    name: String,
    party: String,
    position: String,
}

#[derive(Template)]
#[template(path = "candidates/candidates.html")]
struct CandidatesPage<'a> {
    election_name: String,
    election_uuid: uuid::Uuid,
    search_query: &'a str,
    presidential: &'a [PresidentialTicket],
    council: &'a [CouncilCandidate],
}

pub async fn get_candidate_registration(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Query(search): Query<RegistrationSearch>,
) -> Response {
    trace!(%election_uuid, "Handling candidate registration page request");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let election = match election_registration_state(&state, election_uuid).await {
        Ok(Some(election)) => election,
        Ok(None) => return not_found(),
        Err(response) => return response,
    };
    let election_id: uuid::Uuid = election.get("id");
    let registration_open: bool = election.get("registration_open");
    debug!(%election_uuid, %election_id, citizen_id = citizen.id, registration_open, "Loaded candidate registration context");

    if registration_open {
        if let Err(response) = crate::pages::voting::ensure_snapshot(&state, election_uuid).await {
            return response;
        }
    }
    let eligible = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM election_eligibility WHERE election_id = $1 AND citizen_id = $2)").bind(election_uuid).bind(citizen.uuid).fetch_one(&state.pool).await.unwrap_or(false);
    if registration_open && !eligible && state.app_mode != 0 {
        return bad_request(
            "You're not eligible to register for this election.",
        );
    }
    let mut applicable = match sqlx::query_scalar::<_, String>("SELECT position::text FROM election_positions WHERE election_id = $1 AND position <> 'vice_president' ORDER BY position::text").bind(election_uuid).fetch_all(&state.pool).await { Ok(values) => values, Err(error) => return database_error(error) };
    let active_positions = sqlx::query_scalar::<_, String>("SELECT position::text FROM candidates WHERE election_id = $1 AND citizen_id = $2 AND status = 'active'").bind(election_uuid).bind(citizen.uuid).fetch_all(&state.pool).await.unwrap_or_default();
    applicable.retain(|position| {
        let group = crate::pages::election_lifecycle::position_group(position);
        !active_positions.iter().any(|active| {
            crate::pages::election_lifecycle::position_group(active) == group && active != position
        })
    });
    let selected_position = if applicable
        .iter()
        .any(|position| position == &search.position)
    {
        search.position.clone()
    } else {
        applicable.first().cloned().unwrap_or_default()
    };
    let selected_group =
        crate::pages::election_lifecycle::position_group(&selected_position).unwrap_or(0) as i32;
    let position_options: Vec<RegistrationPosition> = applicable
        .iter()
        .map(|value| RegistrationPosition {
            value: value.clone(),
            label: crate::pages::election_lifecycle::position_label(value).to_string(),
            selected: value == &selected_position,
        })
        .collect();
    let candidate = match sqlx::query(
        "SELECT uuid AS id, position::text AS position, election_display_name, party, status::text AS status
         FROM candidates WHERE election_id = $1 AND citizen_id = $2 AND status = 'active'
         AND CASE WHEN position IN ('president', 'vice_president', 'council', 'ombudsman') THEN 1 ELSE 2 END = $3",
    )
    .bind(election_id)
    .bind(citizen.uuid)
    .bind(selected_group)
    .fetch_optional(&state.pool)
    .await {
        Ok(candidate) => {
            debug!(%election_uuid, citizen_id = citizen.id, registration_found = candidate.is_some(), "Retrieved citizen candidate registration");
            candidate
        }
        Err(error) => return database_error(error),
    };

    let mut page = CandidateRegistrationPage::new();
    match candidate {
        None if registration_open => {
            trace!(%election_uuid, citizen_id = citizen.id, "Rendering new candidate registration form");
            let options = match vp_options(&state, election_id, citizen.uuid, None).await {
                Ok(options) => options,
                Err(response) => return response,
            };
            page.show_form(
                Some(&selected_position),
                &citizen.display_name,
                "",
                options,
                "",
                "",
                vec![String::new(); 5],
                false,
            );
        }
        None => {
            trace!(%election_uuid, citizen_id = citizen.id, "Rendering closed candidate registration state");
            page.show_status(
                "Candidate registration is not currently open.".to_string(),
                false,
            )
        }
        Some(candidate) => {
            let position: String = candidate.get("position");
            let status: String = candidate.get("status");
            debug!(%election_uuid, citizen_id = citizen.id, %position, %status, "Rendering existing candidate registration");
            if position == "vice_president" {
                let ticket = match sqlx::query(
                    "SELECT presidential_tickets.status::text AS status,
                            president.election_display_name AS president_name,
                            vice_president.election_display_name AS vice_president_name
                     FROM presidential_tickets
                     JOIN candidates president ON president.uuid = presidential_tickets.president_candidate_id
                     JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
                     WHERE presidential_tickets.election_id = $1 AND vice_president.uuid = $2",
                )
                .bind(election_id)
                .bind(candidate.get::<uuid::Uuid, _>("id"))
                .fetch_optional(&state.pool)
                .await {
                    Ok(Some(ticket)) => ticket,
                    Ok(None) => return not_found(),
                    Err(error) => return database_error(error),
                };
                page.show_status(format!("You are registered as {}'s vice president under the name {}. Ticket status: {}.", ticket.get::<String, _>("president_name"), ticket.get::<String, _>("vice_president_name"), ticket.get::<String, _>("status")), true);
            } else if status != "active" {
                page.show_status(
                    format!(
                        "Your {position} registration is {status} and can no longer be edited."
                    ),
                    false,
                );
            } else if position != "president" && registration_open {
                page.show_form(
                    Some("council"),
                    candidate.get("election_display_name"),
                    candidate.get("party"),
                    Vec::new(),
                    "",
                    "",
                    vec![String::new(); 5],
                    true,
                );
            } else if position == "president" {
                let ticket = match sqlx::query(
                    "SELECT presidential_tickets.uuid AS id, presidential_tickets.status::text AS status,
                            vice_president.citizen_id AS vice_president_citizen_id,
                            vice_president.election_display_name AS vice_president_name,
                            vice_president.party AS vice_president_party
                     FROM presidential_tickets
                     JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
                     WHERE presidential_tickets.election_id = $1 AND presidential_tickets.president_candidate_id = $2",
                )
                .bind(election_id)
                .bind(candidate.get::<uuid::Uuid, _>("id"))
                .fetch_one(&state.pool)
                .await {
                    Ok(ticket) => ticket,
                    Err(error) => return database_error(error),
                };
                let ticket_status: String = ticket.get("status");
                if ticket_status != "active" {
                    page.show_status(format!("Your presidential ticket is {ticket_status} and can no longer be edited."), false);
                } else if registration_open {
                    let current_vp: uuid::Uuid = ticket.get("vice_president_citizen_id");
                    let options =
                        match vp_options(&state, election_id, citizen.uuid, Some(current_vp)).await
                        {
                            Ok(options) => options,
                            Err(response) => return response,
                        };
                    let messages = match load_messages(&state, ticket.get("id")).await {
                        Ok(messages) => messages,
                        Err(response) => return response,
                    };
                    page.show_form(
                        Some("president"),
                        candidate.get("election_display_name"),
                        candidate.get("party"),
                        options,
                        ticket.get("vice_president_name"),
                        ticket.get("vice_president_party"),
                        messages,
                        true,
                    );
                } else {
                    page.show_status(
                        "Your presidential ticket is active, but candidate registration is closed."
                            .to_string(),
                        true,
                    );
                }
            } else {
                page.show_status(format!("Your {position} registration is active, but candidate registration is closed."), true);
            }
        }
    }

    let position = page.position.as_deref().unwrap_or_default();
    let template = CandidateRegistrationTemplate {
        election_name: election.get("name"),
        election_uuid,
        position,
        page: &page,
        management_visible: citizen.role & (ELECTION_MINISTER | SUPERADMIN) != 0,
        positions: &position_options,
        presidential: position == "president",
        debug_mode: state.app_mode == 0,
    };
    trace!(%election_uuid, citizen_id = citizen.id, show_form = page.show_form, show_status = page.show_status, "Rendering candidate registration page");
    render_template_page(&template, "Candidate registration", jar, &state.pool)
        .await
        .into_response()
}

pub async fn post_candidate_registration(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Form(form): Form<CandidateForm>,
) -> Response {
    trace!(%election_uuid, position = %form.position, "Handling candidate registration update");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    if let Err(message) = validate_candidate_form(&form) {
        warn!(%election_uuid, citizen_id = citizen.id, position = %form.position, validation_error = message, "Rejected invalid candidate registration");
        return bad_request(message);
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => {
            trace!(%election_uuid, citizen_id = citizen.id, "Started candidate registration transaction");
            transaction
        }
        Err(error) => return database_error(error),
    };
    let election = sqlx::query(
        "SELECT uuid AS id, status::text AS status,
                status = 'upcoming' AND CURRENT_TIMESTAMP >= registration_starts_at AND CURRENT_TIMESTAMP < registration_ends_at AS registration_open
         FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_uuid)
    .fetch_optional(&mut *transaction)
    .await;
    let election = match election {
        Ok(Some(election)) if election.get::<bool, _>("registration_open") => election,
        Ok(Some(_)) => {
            warn!(%election_uuid, citizen_id = citizen.id, "Rejected candidate update outside registration period");
            return bad_request("Candidate registration is not currently open.");
        }
        Ok(None) => {
            debug!(%election_uuid, "Candidate registration election was not found");
            return not_found();
        }
        Err(error) => return database_error(error),
    };
    let election_id: uuid::Uuid = election.get("id");
    let debug_bypass = state.app_mode == 0 && form.debug_census;
    if !debug_bypass {
        if let Err(response) = crate::pages::voting::ensure_snapshot(&state, election_uuid).await {
            return response;
        }
    }
    let eligible = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM election_eligibility WHERE election_id = $1 AND citizen_id = $2)").bind(election_uuid).bind(citizen.uuid).fetch_one(&mut *transaction).await.unwrap_or(false);
    if !eligible && !debug_bypass {
        return bad_request(
            "You're not eligible to register for this election.",
        );
    }
    let applicable = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM election_positions WHERE election_id = $1 AND position::text = $2)").bind(election_id).bind(&form.position).fetch_one(&mut *transaction).await.unwrap_or(false);
    if !applicable {
        return bad_request("You cannot register for this position in the election.");
    }
    let existing = sqlx::query(
        "SELECT uuid AS id, position::text AS position, status::text AS status
         FROM candidates WHERE election_id = $1 AND citizen_id = $2 AND status = 'active'
         AND CASE WHEN position IN ('president', 'vice_president', 'council', 'ombudsman') THEN 1 ELSE 2 END = $3 FOR UPDATE",
    )
    .bind(election_id)
    .bind(citizen.uuid)
    .bind(crate::pages::election_lifecycle::position_group(&form.position).unwrap_or(0) as i32)
    .fetch_optional(&mut *transaction)
    .await;
    let existing = match existing {
        Ok(existing) => {
            debug!(%election_uuid, %election_id, citizen_id = citizen.id, existing_registration = existing.is_some(), "Loaded existing candidate registration for update");
            existing
        }
        Err(error) => return database_error(error),
    };

    let result = match existing {
        Some(existing) if existing.get::<String, _>("status") != "active" => {
            warn!(%election_uuid, citizen_id = citizen.id, "Rejected edit of inactive candidate registration");
            return bad_request("A withdrawn or invalidated registration cannot be edited.");
        }
        Some(existing) if existing.get::<String, _>("position") != form.position => {
            warn!(%election_uuid, citizen_id = citizen.id, current_position = %existing.get::<String, _>("position"), requested_position = %form.position, "Rejected candidate role change");
            return bad_request("A candidate cannot change roles after registering.");
        }
        Some(existing) if form.position != "president" && form.position != "vice_president" => {
            debug!(%election_uuid, citizen_id = citizen.id, "Updating council candidate registration");
            sqlx::query(
                "UPDATE candidates SET election_display_name = $1, party = $2 WHERE uuid = $3",
            )
            .bind(form.election_display_name.trim())
            .bind(form.party.trim())
            .bind(existing.get::<uuid::Uuid, _>("id"))
            .execute(&mut *transaction)
            .await
            .map(|_| ())
        }
        Some(existing) if form.position == "president" => {
            debug!(%election_uuid, citizen_id = citizen.id, "Updating presidential ticket registration");
            update_presidential_registration(
                &mut transaction,
                election_id,
                existing.get("id"),
                citizen.uuid,
                &form,
            )
            .await
        }
        Some(_) => {
            warn!(%election_uuid, citizen_id = citizen.id, "Rejected vice president form edit");
            return bad_request("Vice presidents cannot edit the presidential form.");
        }
        None if form.position != "president" && form.position != "vice_president" => {
            debug!(%election_uuid, citizen_id = citizen.id, "Creating council candidate registration");
            sqlx::query(
                "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
                 VALUES ($1, $2, $3::candidate_position, $4, $5)",
            )
            .bind(election_id)
            .bind(citizen.uuid)
            .bind(&form.position)
            .bind(form.election_display_name.trim())
            .bind(form.party.trim())
            .execute(&mut *transaction)
            .await
            .map(|_| ())
        }
        None => {
            debug!(%election_uuid, citizen_id = citizen.id, "Creating presidential ticket registration");
            create_presidential_registration(&mut transaction, election_id, citizen.uuid, &form)
                .await
        }
    };
    if let Err(error) = result {
        return candidate_database_error(error);
    }
    if let Err(error) = transaction.commit().await {
        return database_error(error);
    }
    info!(%election_uuid, %election_id, citizen_id = citizen.id, position = %form.position, "Saved candidate registration");
    Redirect::to(&format!("/elections/{election_uuid}/register")).into_response()
}

pub async fn post_withdraw_candidate(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Query(search): Query<RegistrationSearch>,
) -> Response {
    trace!(%election_uuid, "Handling candidate withdrawal");
    let citizen = match require_citizen(&state, &jar).await {
        Ok(citizen) => citizen,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => {
            trace!(%election_uuid, citizen_id = citizen.id, "Started candidate withdrawal transaction");
            transaction
        }
        Err(error) => return database_error(error),
    };
    let candidate = sqlx::query(
        "SELECT candidates.uuid AS id, candidates.uuid, candidates.position::text AS position,
                candidates.status::text AS status, elections.uuid AS election_id
         FROM candidates JOIN elections ON elections.uuid = candidates.election_id
         WHERE elections.uuid = $1 AND candidates.citizen_id = $2 AND candidates.status = 'active'
         AND CASE WHEN candidates.position IN ('president', 'vice_president', 'council', 'ombudsman') THEN 1 ELSE 2 END = $3
         FOR UPDATE OF candidates",
    )
    .bind(election_uuid)
    .bind(citizen.uuid)
    .bind(crate::pages::election_lifecycle::position_group(&search.position).unwrap_or(0) as i32)
    .fetch_optional(&mut *transaction)
    .await;
    let candidate = match candidate {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            debug!(%election_uuid, citizen_id = citizen.id, "Candidate withdrawal target was not found");
            return not_found();
        }
        Err(error) => return database_error(error),
    };
    let position: String = candidate.get("position");
    debug!(%election_uuid, citizen_id = citizen.id, %position, "Loaded candidate withdrawal target");
    let (target_type, target_uuid, previous) =
        if position != "president" && position != "vice_president" {
            let previous: String = candidate.get("status");
            if previous == "withdrawn" {
                return bad_request("This registration is already withdrawn.");
            }
            if let Err(error) =
                sqlx::query("UPDATE candidates SET status = 'withdrawn' WHERE uuid = $1")
                    .bind(candidate.get::<uuid::Uuid, _>("id"))
                    .execute(&mut *transaction)
                    .await
            {
                return database_error(error);
            }
            ("candidate", candidate.get("uuid"), previous)
        } else {
            let ticket = sqlx::query(
                "SELECT uuid, status::text AS status FROM presidential_tickets
             WHERE president_candidate_id = $1 OR vice_president_candidate_id = $1 FOR UPDATE",
            )
            .bind(candidate.get::<uuid::Uuid, _>("id"))
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
             WHERE uuid IN (
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
    info!(%election_uuid, citizen_id = citizen.id, target_type, %target_uuid, "Withdrew candidate registration");
    Redirect::to(&format!("/elections/{election_uuid}/register")).into_response()
}

pub async fn get_election_candidates(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
    Query(search): Query<CandidateSearch>,
) -> Response {
    trace!(%election_uuid, "Handling public election candidates request");
    let election = match sqlx::query("SELECT uuid AS id, name FROM elections WHERE uuid = $1")
        .bind(election_uuid)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(election)) => election,
        Ok(None) => {
            debug!(%election_uuid, "Public candidates election was not found");
            return not_found();
        }
        Err(error) => return database_error(error),
    };
    let election_id: uuid::Uuid = election.get("id");
    let tickets = match sqlx::query(
        "SELECT presidential_tickets.uuid AS id,
                president.election_display_name AS president_name, president.party AS president_party,
                vice_president.election_display_name AS vice_president_name, vice_president.party AS vice_president_party
         FROM presidential_tickets
         JOIN candidates president ON president.uuid = presidential_tickets.president_candidate_id
         JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
         WHERE presidential_tickets.election_id = $1 AND presidential_tickets.status = 'active'
         AND president.status = 'active' AND vice_president.status = 'active'
         ORDER BY president.election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await {
        Ok(tickets) => {
            debug!(%election_uuid, %election_id, ticket_count = tickets.len(), "Retrieved active presidential tickets");
            tickets
        }
        Err(error) => return database_error(error),
    };
    let mut presidential = Vec::new();
    let search_value = search.q.trim().to_lowercase();
    for ticket in tickets {
        let searchable = format!(
            "{} {} {} {}",
            ticket.get::<String, _>("president_name"),
            ticket.get::<String, _>("president_party"),
            ticket.get::<String, _>("vice_president_name"),
            ticket.get::<String, _>("vice_president_party"),
        )
        .to_lowercase();
        if !search_value.is_empty() && !searchable.contains(&search_value) {
            continue;
        }
        let messages = match load_messages(&state, ticket.get("id")).await {
            Ok(messages) => messages,
            Err(response) => return response,
        };
        let messages = messages
            .into_iter()
            .filter(|message| !message.is_empty())
            .collect();
        presidential.push(PresidentialTicket {
            president_name: ticket.get("president_name"),
            president_party: ticket.get("president_party"),
            vice_president_name: ticket.get("vice_president_name"),
            vice_president_party: ticket.get("vice_president_party"),
            messages,
        });
    }
    let council = match sqlx::query(
        "SELECT election_display_name, party, position::text AS position FROM candidates
         WHERE election_id = $1 AND position NOT IN ('president', 'vice_president') AND status = 'active'
         ORDER BY position::text, election_display_name",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(council) => {
            debug!(%election_uuid, %election_id, candidate_count = council.len(), "Retrieved active council candidates");
            council
        }
        Err(error) => return database_error(error),
    };
    let mut council_items = Vec::new();
    for candidate in council {
        let searchable = format!(
            "{} {}",
            candidate.get::<String, _>("election_display_name"),
            candidate.get::<String, _>("party"),
        )
        .to_lowercase();
        if !search_value.is_empty() && !searchable.contains(&search_value) {
            continue;
        }
        council_items.push(CouncilCandidate {
            name: candidate.get("election_display_name"),
            party: candidate.get("party"),
            position: crate::pages::election_lifecycle::position_label(
                &candidate.get::<String, _>("position"),
            )
            .to_string(),
        });
    }
    let page = CandidatesPage {
        election_name: election.get("name"),
        election_uuid,
        search_query: search.q.trim(),
        presidential: &presidential,
        council: &council_items,
    };
    trace!(%election_uuid, "Rendering public election candidates page");
    render_template_page(&page, "Candidates", jar, &state.pool)
        .await
        .into_response()
}

async fn election_registration_state(
    state: &AppState,
    election_uuid: uuid::Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, Response> {
    trace!(%election_uuid, "Retrieving election registration state");
    sqlx::query(
        "SELECT uuid AS id, name, status::text AS status,
                status = 'upcoming' AND CURRENT_TIMESTAMP >= registration_starts_at AND CURRENT_TIMESTAMP < registration_ends_at AS registration_open
         FROM elections WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)
}

async fn vp_options(
    state: &AppState,
    election_id: uuid::Uuid,
    president_citizen_id: uuid::Uuid,
    current_vp: Option<uuid::Uuid>,
) -> Result<Vec<VicePresidentOption>, Response> {
    trace!(
        %election_id,
        %president_citizen_id, ?current_vp, "Retrieving eligible vice presidents"
    );
    let citizens = sqlx::query(
        "SELECT citizens.uuid AS id,
                COALESCE(NULLIF(authentik_identities.display_name, ''),
                         NULLIF(authentik_identities.preferred_username, ''),
                         NULLIF(authentik_identities.email, ''),
                         'Citizen ' || citizens.uuid::text) AS display_name
         FROM citizens JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
         WHERE citizens.banned = FALSE AND citizens.uuid <> $2
         AND (citizens.uuid = $3 OR NOT EXISTS (
             SELECT 1 FROM candidates
             WHERE candidates.election_id = $1 AND candidates.citizen_id = citizens.uuid
             AND candidates.status = 'active'
         )) ORDER BY display_name",
    )
    .bind(election_id)
    .bind(president_citizen_id)
    .bind(current_vp.unwrap_or(uuid::Uuid::nil()))
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    debug!(
        %election_id,
        citizen_count = citizens.len(),
        "Retrieved eligible vice presidents"
    );
    let mut options = Vec::new();
    for citizen in citizens {
        let id: uuid::Uuid = citizen.get("id");
        options.push(VicePresidentOption {
            id,
            selected: Some(id) == current_vp,
            display_name: citizen.get("display_name"),
        });
    }
    Ok(options)
}

fn validate_candidate_form(form: &CandidateForm) -> Result<(), &'static str> {
    trace!(position = %form.position, "Validating candidate registration form");
    if !crate::pages::election_lifecycle::DIRECT_POSITIONS
        .iter()
        .any(|(position, _)| *position == form.position)
    {
        return Err("A valid position is required.");
    }
    if form.election_display_name.trim().is_empty() || form.party.trim().is_empty() {
        return Err("Display name and party are required.");
    }
    if form.position == "president" {
        if form
            .vice_president_citizen_id
            .parse::<uuid::Uuid>()
            .is_err()
            || form.vice_president_display_name.trim().is_empty()
            || form.vice_president_party.trim().is_empty()
        {
            return Err("A vice president, their display name, and their party are required.");
        }
        validate_messages(&form_messages(form))?;
    }
    trace!(position = %form.position, "Candidate registration validation succeeded");
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
    election_id: uuid::Uuid,
    president_citizen_id: uuid::Uuid,
    form: &CandidateForm,
) -> Result<(), sqlx::Error> {
    let vp_citizen_id = form
        .vice_president_citizen_id
        .parse::<uuid::Uuid>()
        .unwrap();
    trace!(
        %election_id,
        %president_citizen_id, %vp_citizen_id, "Creating presidential registration"
    );
    ensure_vp_available(
        transaction,
        election_id,
        president_citizen_id,
        vp_citizen_id,
        None,
    )
    .await?;
    let president_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
         VALUES ($1, $2, 'president', $3, $4) RETURNING uuid",
    )
    .bind(election_id)
    .bind(president_citizen_id)
    .bind(form.election_display_name.trim())
    .bind(form.party.trim())
    .fetch_one(&mut **transaction)
    .await?;
    let vp_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
         VALUES ($1, $2, 'vice_president', $3, $4) RETURNING uuid",
    )
    .bind(election_id)
    .bind(vp_citizen_id)
    .bind(form.vice_president_display_name.trim())
    .bind(form.vice_president_party.trim())
    .fetch_one(&mut **transaction)
    .await?;
    let ticket_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO presidential_tickets (election_id, president_candidate_id, vice_president_candidate_id)
         VALUES ($1, $2, $3) RETURNING uuid",
    )
    .bind(election_id)
    .bind(president_id)
    .bind(vp_id)
    .fetch_one(&mut **transaction)
    .await?;
    debug!(
        %election_id,
        %president_id, %vp_id, %ticket_id, "Created presidential ticket records"
    );
    replace_messages(transaction, ticket_id, &form_messages(form)).await
}

async fn update_presidential_registration(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    president_id: uuid::Uuid,
    president_citizen_id: uuid::Uuid,
    form: &CandidateForm,
) -> Result<(), sqlx::Error> {
    trace!(
        %election_id,
        %president_id, %president_citizen_id, "Updating presidential registration"
    );
    let ticket = sqlx::query(
        "SELECT presidential_tickets.uuid AS id, presidential_tickets.status::text AS status,
                vice_president_candidate_id, vice_president.citizen_id AS old_vp_citizen_id
         FROM presidential_tickets
         JOIN candidates vice_president ON vice_president.uuid = presidential_tickets.vice_president_candidate_id
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
    let vp_citizen_id = form
        .vice_president_citizen_id
        .parse::<uuid::Uuid>()
        .unwrap();
    let old_vp_citizen_id: uuid::Uuid = ticket.get("old_vp_citizen_id");
    ensure_vp_available(
        transaction,
        election_id,
        president_citizen_id,
        vp_citizen_id,
        Some(old_vp_citizen_id),
    )
    .await?;
    let mut vp_id: uuid::Uuid = ticket.get("vice_president_candidate_id");
    if vp_citizen_id != old_vp_citizen_id {
        debug!(
            %election_id,
            %president_id,
            %old_vp_citizen_id,
            new_vp_citizen_id = %vp_citizen_id,
            "Replacing presidential ticket vice president"
        );
        let new_vp_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO candidates (election_id, citizen_id, position, election_display_name, party)
             VALUES ($1, $2, 'vice_president', $3, $4) RETURNING uuid",
        )
        .bind(election_id)
        .bind(vp_citizen_id)
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query(
            "UPDATE presidential_tickets SET vice_president_candidate_id = $1 WHERE uuid = $2",
        )
        .bind(new_vp_id)
        .bind(ticket.get::<uuid::Uuid, _>("id"))
        .execute(&mut **transaction)
        .await?;
        sqlx::query("UPDATE candidates SET status = 'invalidated' WHERE uuid = $1")
            .bind(vp_id)
            .execute(&mut **transaction)
            .await?;
        vp_id = new_vp_id;
    }
    sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE uuid = $3")
        .bind(form.election_display_name.trim())
        .bind(form.party.trim())
        .bind(president_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("UPDATE candidates SET election_display_name = $1, party = $2 WHERE uuid = $3")
        .bind(form.vice_president_display_name.trim())
        .bind(form.vice_president_party.trim())
        .bind(vp_id)
        .execute(&mut **transaction)
        .await?;
    replace_messages(transaction, ticket.get("id"), &form_messages(form)).await
}

async fn ensure_vp_available(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    president_citizen_id: uuid::Uuid,
    vp_citizen_id: uuid::Uuid,
    current_vp: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    trace!(
        %election_id,
        %president_citizen_id, %vp_citizen_id, ?current_vp, "Checking vice president availability"
    );
    if president_citizen_id == vp_citizen_id {
        return Err(sqlx::Error::Protocol(
            "The president cannot also be the vice president.".to_string(),
        ));
    }
    let available = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM citizens JOIN authentik_identities ON authentik_identities.citizen_id = citizens.uuid
            WHERE citizens.uuid = $1 AND citizens.banned = FALSE
            AND ($1 = $3 OR NOT EXISTS (
                SELECT 1 FROM candidates
                WHERE candidates.election_id = $2 AND candidates.citizen_id = $1
                AND candidates.status = 'active'
            ))
         )",
    )
    .bind(vp_citizen_id)
    .bind(election_id)
    .bind(current_vp.unwrap_or(uuid::Uuid::nil()))
    .fetch_one(&mut **transaction)
    .await?;
    if !available {
        warn!(
            %election_id,
            %vp_citizen_id, "Requested vice president is unavailable"
        );
        return Err(sqlx::Error::Protocol(
            "That vice president is no longer available.".to_string(),
        ));
    }
    trace!(%election_id, %vp_citizen_id, "Vice president is available");
    Ok(())
}

async fn load_messages(state: &AppState, ticket_id: uuid::Uuid) -> Result<Vec<String>, Response> {
    trace!(%ticket_id, "Loading presidential ticket messages");
    let rows = sqlx::query("SELECT position, message FROM presidential_ticket_messages WHERE presidential_ticket_id = $1 ORDER BY position")
        .bind(ticket_id)
        .fetch_all(&state.pool)
        .await
        .map_err(database_error)?;
    let mut messages = vec![String::new(); 5];
    for row in rows {
        messages[(row.get::<i32, _>("position") - 1) as usize] = row.get("message");
    }
    debug!(
        %ticket_id,
        message_count = messages
            .iter()
            .filter(|message| !message.is_empty())
            .count(),
        "Loaded presidential ticket messages"
    );
    Ok(messages)
}

async fn replace_messages(
    transaction: &mut Transaction<'_, Postgres>,
    ticket_id: uuid::Uuid,
    messages: &[String],
) -> Result<(), sqlx::Error> {
    trace!(
        %ticket_id,
        message_count = messages.len(),
        "Replacing presidential ticket messages"
    );
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
    debug!(
        %ticket_id,
        message_count = messages.len(),
        "Replaced presidential ticket messages"
    );
    Ok(())
}

async fn insert_withdrawal_change(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    actor: &AuthenticatedCitizen,
    target_type: &str,
    target_uuid: uuid::Uuid,
    previous: &str,
) -> Result<(), sqlx::Error> {
    trace!(%election_id, actor_citizen_id = actor.id, target_type, %target_uuid, "Recording candidate withdrawal change");
    sqlx::query(
        "INSERT INTO election_change_log (
            election_id, actor_citizen_id, actor_display_name, target_type, target_uuid,
            previous_value, new_value, reason
         ) VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn', 'Candidate withdrew')",
    )
    .bind(election_id)
    .bind(actor.uuid)
    .bind(&actor.display_name)
    .bind(target_type)
    .bind(target_uuid)
    .bind(previous)
    .execute(&mut **transaction)
    .await?;
    debug!(%election_id, actor_citizen_id = actor.id, target_type, %target_uuid, "Recorded candidate withdrawal change");
    Ok(())
}

fn candidate_database_error(error: sqlx::Error) -> Response {
    match &error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            warn!("Candidate registration violated election role uniqueness");
            bad_request("That user already has a role in this election.")
        }
        sqlx::Error::Protocol(message) => {
            warn!(
                validation_error = message,
                "Candidate registration transaction was rejected"
            );
            bad_request(message)
        }
        _ => database_error(error),
    }
}

fn bad_request(message: &str) -> Response {
    debug!(
        validation_error = message,
        "Returning invalid candidate response"
    );
    error_response(
        StatusCode::BAD_REQUEST,
        &ErrorPage::new("Invalid registration", message, "invalid-registration-page"),
    )
}

fn not_found() -> Response {
    debug!("Returning candidate not found response");
    error_response(
        StatusCode::NOT_FOUND,
        &ErrorPage::new(
            "Election or registration not found",
            "",
            "registration-not-found-page",
        ),
    )
}

fn database_error(error: sqlx::Error) -> Response {
    error!(?error, "Failed to manage candidate registration");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &ErrorPage::new(
            "Could not manage candidate registration",
            "",
            "candidate-registration-error-page",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(position: &str) -> CandidateForm {
        CandidateForm {
            position: position.to_string(),
            election_display_name: "Name".to_string(),
            party: "Party".to_string(),
            vice_president_citizen_id: "00000000-0000-0000-0000-000000000002".to_string(),
            vice_president_display_name: "VP".to_string(),
            vice_president_party: "VP Party".to_string(),
            message_1: String::new(),
            message_2: String::new(),
            message_3: String::new(),
            message_4: String::new(),
            message_5: String::new(),
            debug_census: false,
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
