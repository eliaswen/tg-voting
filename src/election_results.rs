use crate::counting::{PresidentialCount, approval_count, instant_runoff_with_decisions, plurality_count};
use crate::pages::login::AppState;
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, HashMap};
use tracing::{error, info};

struct CandidateResult {
    id: uuid::Uuid,
    votes: i32,
    elected: bool,
    tied: bool,
}

pub async fn count_due_elections(state: &AppState) {
    let elections = match sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT uuid FROM elections
         WHERE status IN ('upcoming', 'voting') AND voting_ends_at <= clock_timestamp()",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(value) => value,
        Err(error) => {
            error!(?error, "Could not find elections due for counting");
            return;
        }
    };
    for election_id in elections {
        if let Err(error) = count_election(state, election_id).await {
            error!(?error, %election_id, "Automatic count failed");
        }
    }
}

pub async fn recount_president_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    count_president(transaction, election_id, revote_id).await
}

async fn count_election(state: &AppState, election_id: uuid::Uuid) -> Result<(), sqlx::Error> {
    let mut transaction = state.pool.begin().await?;
    let election = sqlx::query(
        "SELECT status::text AS status, active_revote_id, voting_ends_at,
                clock_timestamp() >= voting_ends_at AS due
         FROM elections WHERE uuid = $1 FOR UPDATE",
    )
    .bind(election_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(election) = election else { return Ok(()); };
    let status: String = election.get("status");
    if !matches!(status.as_str(), "upcoming" | "voting") || !election.get::<bool, _>("due") {
        return Ok(());
    }
    let revote_id: Option<uuid::Uuid> = election.get("active_revote_id");
    let positions = sqlx::query_scalar::<_, String>(
        "SELECT positions.position::text
         FROM election_positions positions
         LEFT JOIN election_revotes revotes ON revotes.uuid = $2
         WHERE positions.election_id = $1
           AND positions.position IN ('president', 'council', 'ombudsman')
           AND (revotes.mode IS NULL OR revotes.mode = 'all_contests'
                OR positions.position = revotes.contest)
         ORDER BY positions.position::text",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut *transaction)
    .await?;
    for position in positions {
        match position.as_str() {
            "president" => count_president(&mut transaction, election_id, revote_id).await?,
            "council" => count_council(&mut transaction, election_id, revote_id).await?,
            "ombudsman" => count_ombudsman(&mut transaction, election_id, revote_id).await?,
            _ => unreachable!(),
        }
    }
    sqlx::query("UPDATE elections SET status = 'counting' WHERE uuid = $1")
        .bind(election_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    info!(%election_id, "Automatic election count completed; minister review is required");
    Ok(())
}

async fn ballot_ids(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<Vec<uuid::Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT uuid FROM ballots
         WHERE election_id = $1 AND revote_id IS NOT DISTINCT FROM $2
         ORDER BY uuid",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut **transaction)
    .await
}

async fn presidential_tickets(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<Vec<uuid::Uuid>, sqlx::Error> {
    if let Some(revote_id) = revote_id {
        sqlx::query_scalar("SELECT ticket_id FROM election_revote_tickets WHERE revote_id = $1 ORDER BY ticket_id")
            .bind(revote_id).fetch_all(&mut **transaction).await
    } else {
        sqlx::query_scalar("SELECT uuid FROM presidential_tickets WHERE election_id = $1 AND status = 'active' ORDER BY uuid")
            .bind(election_id).fetch_all(&mut **transaction).await
    }
}

async fn contest_candidates(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
    contest: &str,
) -> Result<Vec<uuid::Uuid>, sqlx::Error> {
    if let Some(revote_id) = revote_id {
        sqlx::query_scalar(
            "SELECT candidate_id FROM election_revote_candidates
             WHERE revote_id = $1 AND contest = $2::candidate_position ORDER BY candidate_id",
        )
        .bind(revote_id)
        .bind(contest)
        .fetch_all(&mut **transaction)
        .await
    } else {
        sqlx::query_scalar(
            "SELECT uuid FROM candidates
             WHERE election_id = $1 AND position = $2::candidate_position AND status = 'active'
             ORDER BY uuid",
        )
        .bind(election_id)
        .bind(contest)
        .fetch_all(&mut **transaction)
        .await
    }
}

async fn count_president(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    let tickets = presidential_tickets(transaction, election_id, revote_id).await?;
    let ballot_ids = ballot_ids(transaction, election_id, revote_id).await?;
    let mut grouped: BTreeMap<_, Vec<uuid::Uuid>> = ballot_ids
        .iter().copied().map(|id| (id, Vec::new())).collect();
    for row in sqlx::query(
        "SELECT presidential_votes.ballot_uuid, presidential_votes.ticket_id
         FROM presidential_votes JOIN ballots ON ballots.uuid = presidential_votes.ballot_uuid
         WHERE ballots.election_id = $1 AND ballots.revote_id IS NOT DISTINCT FROM $2
         ORDER BY presidential_votes.ballot_uuid, presidential_votes.ranking",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut **transaction)
    .await?
    {
        grouped.entry(row.get("ballot_uuid")).or_default().push(row.get("ticket_id"));
    }
    let decisions = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT actions.ticket_id FROM election_result_actions actions
         JOIN election_results results ON results.uuid = actions.result_id
         WHERE results.election_id = $1 AND results.contest = 'president'
           AND results.revote_id IS NOT DISTINCT FROM $2
           AND actions.action = 'eliminate' AND actions.ticket_id IS NOT NULL
         ORDER BY actions.created_at, actions.uuid",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut **transaction)
    .await?;
    let count = instant_runoff_with_decisions(&grouped.into_values().collect::<Vec<_>>(), &tickets, &decisions);
    let (status, winner, tied) = match &count.outcome {
        PresidentialCount::Winner(ticket) => ("provisional", Some(*ticket), Vec::new()),
        PresidentialCount::EliminationTie(tied) => ("review", None, tied.clone()),
        PresidentialCount::Runoff(tied) => ("runoff", None, tied.clone()),
    };
    let final_totals = count.rounds.last().map(|round| round.totals.clone()).unwrap_or_default();
    let details = serde_json::json!({
        "method": "instant-runoff",
        "ballots": ballot_ids.len(),
        "rounds": count.rounds,
        "tied": tied,
    });
    let result_id = store_result(transaction, election_id, revote_id, "president", status, &details).await?;
    sqlx::query("DELETE FROM election_result_tickets WHERE result_id = $1")
        .bind(result_id).execute(&mut **transaction).await?;
    for ticket in tickets {
        sqlx::query(
            "INSERT INTO election_result_tickets (result_id, ticket_id, votes, elected, tied)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(result_id)
        .bind(ticket)
        .bind(final_totals.get(&ticket).copied().unwrap_or(0) as i32)
        .bind(winner == Some(ticket))
        .bind(tied.contains(&ticket))
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn count_council(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    let ballot_ids = ballot_ids(transaction, election_id, revote_id).await?;
    let mut grouped: BTreeMap<_, Vec<uuid::Uuid>> = ballot_ids
        .iter().copied().map(|id| (id, Vec::new())).collect();
    for row in sqlx::query(
        "SELECT candidate_votes.ballot_uuid, candidate_votes.candidate_id
         FROM candidate_votes JOIN ballots ON ballots.uuid = candidate_votes.ballot_uuid
         WHERE ballots.election_id = $1 AND ballots.revote_id IS NOT DISTINCT FROM $2
           AND candidate_votes.position = 'council' ORDER BY candidate_votes.ballot_uuid",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut **transaction)
    .await?
    {
        grouped.entry(row.get("ballot_uuid")).or_default().push(row.get("candidate_id"));
    }
    let candidates = contest_candidates(transaction, election_id, revote_id, "council").await?;
    let mut carried = Vec::new();
    let mut seats = sqlx::query_scalar::<_, i32>("SELECT council_seats FROM elections WHERE uuid = $1")
        .bind(election_id).fetch_one(&mut **transaction).await? as usize;
    if let Some(revote_id) = revote_id {
        if let Some(row) = sqlx::query(
            "SELECT source_result_id, unresolved_seats FROM election_revotes
             WHERE uuid = $1 AND mode = 'runoff' AND contest = 'council'",
        )
        .bind(revote_id)
        .fetch_optional(&mut **transaction)
        .await?
        {
            seats = row.get::<i32, _>("unresolved_seats") as usize;
            let source_result_id: uuid::Uuid = row.get("source_result_id");
            carried = sqlx::query(
                "SELECT candidate_id, votes FROM election_result_candidates
                 WHERE result_id = $1 AND elected ORDER BY candidate_id",
            )
            .bind(source_result_id)
            .fetch_all(&mut **transaction)
            .await?
            .into_iter()
            .map(|row| CandidateResult { id: row.get("candidate_id"), votes: row.get("votes"), elected: true, tied: false })
            .collect();
        }
    }
    let ballots = grouped.into_values().collect::<Vec<_>>();
    let count = approval_count(&ballots, &candidates, seats);
    let mut rows = carried;
    rows.extend(candidates.iter().map(|candidate| CandidateResult {
        id: *candidate,
        votes: count.totals.get(candidate).copied().unwrap_or(0) as i32,
        elected: count.winners.contains(candidate),
        tied: count.tied.contains(candidate),
    }));
    let abstained = ballots.iter().filter(|ballot| ballot.is_empty()).count();
    let all_winners: Vec<_> = rows.iter().filter(|row| row.elected).map(|row| row.id).collect();
    let details = serde_json::json!({
        "method": "approval",
        "seats_in_round": seats,
        "ballots": ballots.len(),
        "abstained": abstained,
        "winners": all_winners,
        "tied": count.tied,
        "unresolved_seats": count.unresolved_seats,
    });
    let status = if count.tied.is_empty() { "provisional" } else { "runoff" };
    let result_id = store_result(transaction, election_id, revote_id, "council", status, &details).await?;
    store_candidate_rows(transaction, result_id, &rows).await
}

async fn count_ombudsman(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    let ballot_ids = ballot_ids(transaction, election_id, revote_id).await?;
    let mut choices: HashMap<uuid::Uuid, Option<uuid::Uuid>> =
        ballot_ids.iter().copied().map(|id| (id, None)).collect();
    for row in sqlx::query(
        "SELECT candidate_votes.ballot_uuid, candidate_votes.candidate_id
         FROM candidate_votes JOIN ballots ON ballots.uuid = candidate_votes.ballot_uuid
         WHERE ballots.election_id = $1 AND ballots.revote_id IS NOT DISTINCT FROM $2
           AND candidate_votes.position = 'ombudsman'",
    )
    .bind(election_id)
    .bind(revote_id)
    .fetch_all(&mut **transaction)
    .await?
    {
        choices.insert(row.get("ballot_uuid"), Some(row.get("candidate_id")));
    }
    let candidates = contest_candidates(transaction, election_id, revote_id, "ombudsman").await?;
    let ballot_choices = choices.into_values().collect::<Vec<_>>();
    let count = plurality_count(&ballot_choices, &candidates);
    let tied = if count.winners.len() == 1 { Vec::new() } else { count.winners.clone() };
    let rows: Vec<_> = candidates.iter().map(|candidate| CandidateResult {
        id: *candidate,
        votes: count.totals.get(candidate).copied().unwrap_or(0) as i32,
        elected: count.winners.len() == 1 && count.winners[0] == *candidate,
        tied: tied.contains(candidate),
    }).collect();
    let details = serde_json::json!({
        "method": "plurality",
        "ballots": ballot_choices.len(),
        "abstained": ballot_choices.iter().filter(|choice| choice.is_none()).count(),
        "tied": tied,
    });
    let status = if count.winners.len() == 1 { "provisional" } else { "runoff" };
    let result_id = store_result(transaction, election_id, revote_id, "ombudsman", status, &details).await?;
    store_candidate_rows(transaction, result_id, &rows).await
}

async fn store_result(
    transaction: &mut Transaction<'_, Postgres>,
    election_id: uuid::Uuid,
    revote_id: Option<uuid::Uuid>,
    contest: &str,
    status: &str,
    details: &serde_json::Value,
) -> Result<uuid::Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO election_results
             (election_id, contest, status, revote_id, round_number, count_details)
         VALUES ($1, $2::candidate_position, $3, $4,
                 COALESCE((SELECT max(round_number) + 1 FROM election_results
                           WHERE election_id = $1 AND contest = $2::candidate_position), 1), $5)
         ON CONFLICT (election_id, contest, revote_id) DO UPDATE
         SET status = EXCLUDED.status, calculated_at = clock_timestamp(),
             count_details = EXCLUDED.count_details,
             reviewed_at = NULL, reviewed_by = NULL, review_reason = NULL
         WHERE election_results.status <> 'certified' AND election_results.reviewed_at IS NULL
         RETURNING uuid",
    )
    .bind(election_id)
    .bind(contest)
    .bind(status)
    .bind(revote_id)
    .bind(details)
    .fetch_one(&mut **transaction)
    .await
}

async fn store_candidate_rows(
    transaction: &mut Transaction<'_, Postgres>,
    result_id: uuid::Uuid,
    rows: &[CandidateResult],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM election_result_candidates WHERE result_id = $1")
        .bind(result_id).execute(&mut **transaction).await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO election_result_candidates (result_id, candidate_id, votes, elected, tied)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(result_id).bind(row.id).bind(row.votes).bind(row.elected).bind(row.tied)
        .execute(&mut **transaction).await?;
    }
    Ok(())
}
