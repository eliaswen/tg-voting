use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::fmt::Write;

use crate::{
    pages::{auth::require_election_manager, login::AppState},
    render::render_template_page,
};

#[derive(Template)]
#[template(path = "manage-elections/posts.html")]
struct ElectionPostsPage<'a> {
    election_uuid: uuid::Uuid,
    election_name: &'a str,
    registration_discord: &'a str,
    registration_reddit: &'a str,
    voting_discord: &'a str,
    voting_reddit: &'a str,
    results_ready: bool,
    results_discord: &'a str,
    results_reddit: &'a str,
}

#[derive(Clone, Copy)]
enum Platform {
    Discord,
    Reddit,
}

struct ElectionInfo {
    name: String,
    status: String,
    registration_ends_at: DateTime<Utc>,
    voting_ends_at: DateTime<Utc>,
}

#[derive(Clone)]
struct Identity {
    election_name: String,
    discord_username: Option<String>,
    reddit_username: Option<String>,
}

impl Identity {
    fn for_platform(&self, platform: Platform) -> String {
        let discord = self
            .discord_username
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let reddit = self
            .reddit_username
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        match platform {
            Platform::Discord => discord
                .map(discord_name)
                .or_else(|| reddit.map(reddit_name)),
            Platform::Reddit => reddit
                .map(reddit_name)
                .or_else(|| discord.map(discord_name)),
        }
        .unwrap_or_else(|| self.election_name.clone())
    }
}

struct ResultOption {
    identity: Identity,
    running_mate: Option<Identity>,
    votes: i32,
    elected: bool,
}

struct ContestResult {
    contest: String,
    ballots: i64,
    options: Vec<ResultOption>,
}

pub async fn get_election_posts(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(election_uuid): Path<uuid::Uuid>,
) -> Response {
    if let Err(response) = require_election_manager(&state, &jar).await {
        return response;
    }
    let row = match sqlx::query(
        "SELECT name, status::text AS status, registration_ends_at, voting_ends_at
         FROM elections WHERE uuid = $1",
    )
    .bind(election_uuid)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let (Some(registration_ends_at), Some(voting_ends_at)) = (
        row.get::<Option<DateTime<Utc>>, _>("registration_ends_at"),
        row.get::<Option<DateTime<Utc>>, _>("voting_ends_at"),
    ) else {
        return (
            StatusCode::CONFLICT,
            "Set the registration and voting schedules before generating posts.",
        )
            .into_response();
    };
    let election = ElectionInfo {
        name: row.get("name"),
        status: row.get("status"),
        registration_ends_at,
        voting_ends_at,
    };
    let host = state.public_host.trim_end_matches('/');
    let registration_discord = registration_post(&election, election_uuid, host, Platform::Discord);
    let registration_reddit = registration_post(&election, election_uuid, host, Platform::Reddit);
    let voting_discord = voting_post(&election, election_uuid, host, Platform::Discord);
    let voting_reddit = voting_post(&election, election_uuid, host, Platform::Reddit);
    let results_ready = election.status == "certified";
    let (results_discord, results_reddit) = if results_ready {
        let results = match load_results(&state, election_uuid).await {
            Ok(results) => results,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        (
            results_post(&election, election_uuid, host, Platform::Discord, &results),
            results_post(&election, election_uuid, host, Platform::Reddit, &results),
        )
    } else {
        (String::new(), String::new())
    };
    render_template_page(
        &ElectionPostsPage {
            election_uuid,
            election_name: &election.name,
            registration_discord: &registration_discord,
            registration_reddit: &registration_reddit,
            voting_discord: &voting_discord,
            voting_reddit: &voting_reddit,
            results_ready,
            results_discord: &results_discord,
            results_reddit: &results_reddit,
        },
        "Election announcement posts",
        jar,
        &state.pool,
    )
    .await
    .into_response()
}

fn registration_post(
    election: &ElectionInfo,
    id: uuid::Uuid,
    host: &str,
    platform: Platform,
) -> String {
    format!(
        "# Applications for the {} are now open\n\nApplications will stay open until {}.\n\n[APPLY]({host}/elections/{id}/register)\n[CANDIDATES]({host}/elections/{id}/candidates)",
        election.name,
        deadline(election.registration_ends_at, platform),
    )
}

fn voting_post(election: &ElectionInfo, id: uuid::Uuid, host: &str, platform: Platform) -> String {
    format!(
        "# {} are now open\n\n[VOTE]({host}/elections/{id}/vote)\n[LIVE VOTES]({host}/elections/{id})\n[CANDIDATES]({host}/elections/{id}/candidates)\n\nThe vote will be open until {}.",
        election.name,
        deadline(election.voting_ends_at, platform),
    )
}

fn deadline(value: DateTime<Utc>, platform: Platform) -> String {
    match platform {
        Platform::Discord => format!("<t:{}>, <t:{}:R>", value.timestamp(), value.timestamp()),
        Platform::Reddit => value.format("%B %-d, %Y at %H:%M UTC").to_string(),
    }
}

fn discord_name(value: &str) -> String {
    format!("@{}", value.trim().trim_start_matches('@'))
}
fn reddit_name(value: &str) -> String {
    format!(
        "u/{}",
        value
            .trim()
            .trim_start_matches("u/")
            .trim_start_matches('/')
    )
}

async fn load_results(
    state: &AppState,
    election_id: uuid::Uuid,
) -> Result<Vec<ContestResult>, sqlx::Error> {
    let result_rows = sqlx::query(
        "SELECT DISTINCT ON (contest) uuid, contest::text AS contest,
                COALESCE((count_details->>'ballots')::bigint, 0) AS ballots
         FROM election_results WHERE election_id = $1 AND status = 'certified'
         ORDER BY contest, round_number DESC",
    )
    .bind(election_id)
    .fetch_all(&state.pool)
    .await?;
    let mut results = Vec::new();
    for result in result_rows {
        let result_id: uuid::Uuid = result.get("uuid");
        let contest: String = result.get("contest");
        let rows = if contest == "president" {
            sqlx::query(
                "SELECT president.election_display_name, pd.discord_username, pr.reddit_username,
                        vice.election_display_name AS mate_name, vd.discord_username AS mate_discord,
                        vr.reddit_username AS mate_reddit, options.votes, options.elected
                 FROM election_result_tickets options
                 JOIN presidential_tickets tickets ON tickets.uuid = options.ticket_id
                 JOIN candidates president ON president.uuid = tickets.president_candidate_id
                 JOIN candidates vice ON vice.uuid = tickets.vice_president_candidate_id
                 LEFT JOIN citizen_discord_links pd ON pd.citizen_id = president.citizen_id
                 LEFT JOIN citizen_reddit_links pr ON pr.citizen_id = president.citizen_id
                 LEFT JOIN citizen_discord_links vd ON vd.citizen_id = vice.citizen_id
                 LEFT JOIN citizen_reddit_links vr ON vr.citizen_id = vice.citizen_id
                 WHERE options.result_id = $1 ORDER BY options.votes DESC, president.election_display_name",
            ).bind(result_id).fetch_all(&state.pool).await?
        } else {
            sqlx::query(
                "SELECT candidates.election_display_name, discord.discord_username, reddit.reddit_username,
                        NULL::text AS mate_name, NULL::text AS mate_discord, NULL::text AS mate_reddit,
                        options.votes, options.elected
                 FROM election_result_candidates options
                 JOIN candidates ON candidates.uuid = options.candidate_id
                 LEFT JOIN citizen_discord_links discord ON discord.citizen_id = candidates.citizen_id
                 LEFT JOIN citizen_reddit_links reddit ON reddit.citizen_id = candidates.citizen_id
                 WHERE options.result_id = $1 ORDER BY options.votes DESC, candidates.election_display_name",
            ).bind(result_id).fetch_all(&state.pool).await?
        };
        let options = rows
            .into_iter()
            .map(|row| ResultOption {
                identity: Identity {
                    election_name: row.get("election_display_name"),
                    discord_username: row.get("discord_username"),
                    reddit_username: row.get("reddit_username"),
                },
                running_mate: row
                    .get::<Option<String>, _>("mate_name")
                    .map(|name| Identity {
                        election_name: name,
                        discord_username: row.get("mate_discord"),
                        reddit_username: row.get("mate_reddit"),
                    }),
                votes: row.get("votes"),
                elected: row.get("elected"),
            })
            .collect();
        results.push(ContestResult {
            contest,
            ballots: result.get("ballots"),
            options,
        });
    }
    Ok(results)
}

fn results_post(
    election: &ElectionInfo,
    id: uuid::Uuid,
    host: &str,
    platform: Platform,
    results: &[ContestResult],
) -> String {
    let mut post = format!("# {} Results\n", election.name);
    for result in results {
        let title = match result.contest.as_str() {
            "president" => "President",
            "council" => "Council",
            "ombudsman" => "Ombudsman",
            other => other,
        };
        let _ = write!(
            post,
            "\n## {title} Results\n\nA total of {} valid ballots were counted for this contest.\n\n",
            result.ballots
        );
        for (index, option) in result.options.iter().enumerate() {
            let mut name = option.identity.for_platform(platform);
            if let Some(mate) = &option.running_mate {
                name.push_str(" / ");
                name.push_str(&mate.for_platform(platform));
            }
            let elected = if option.elected { " — elected" } else { "" };
            let _ = writeln!(
                post,
                "{}. {} - {} votes{}",
                index + 1,
                name,
                option.votes,
                elected
            );
        }
        let winners: Vec<String> = result
            .options
            .iter()
            .filter(|option| option.elected)
            .map(|option| {
                let mut name = option.identity.for_platform(platform);
                if let Some(mate) = &option.running_mate {
                    name.push_str(" / ");
                    name.push_str(&mate.for_platform(platform));
                }
                name
            })
            .collect();
        if !winners.is_empty() {
            let label = if winners.len() == 1 {
                "Winner"
            } else {
                "Elected"
            };
            let _ = write!(post, "\n{label}: {}\n", winners.join(", "));
        }
    }
    let _ = write!(
        post,
        "\n[VIEW RESULTS]({host}/elections/{id}/results)\n[VIEW BALLOT RECEIPTS]({host}/elections/{id}/receipts)"
    );
    post
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_fall_back_to_the_other_link() {
        let reddit_only = Identity {
            election_name: "Display".into(),
            discord_username: None,
            reddit_username: Some("citizen".into()),
        };
        let discord_only = Identity {
            election_name: "Display".into(),
            discord_username: Some("citizen".into()),
            reddit_username: None,
        };
        assert_eq!(reddit_only.for_platform(Platform::Discord), "u/citizen");
        assert_eq!(discord_only.for_platform(Platform::Reddit), "@citizen");
    }

    #[test]
    fn discord_and_reddit_deadlines_are_platform_specific() {
        let value = DateTime::from_timestamp(1_789_895_700, 0).unwrap();
        assert_eq!(
            deadline(value, Platform::Discord),
            "<t:1789895700>, <t:1789895700:R>"
        );
        assert!(deadline(value, Platform::Reddit).ends_with(" UTC"));
    }
}
