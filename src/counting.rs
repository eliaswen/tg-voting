use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresidentialCount {
    Winner(uuid::Uuid),
    EliminationTie(Vec<uuid::Uuid>),
    Runoff(Vec<uuid::Uuid>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CountRound {
    pub number: usize,
    pub totals: BTreeMap<uuid::Uuid, usize>,
    pub exhausted: usize,
    pub eliminated: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedCount {
    pub outcome: PresidentialCount,
    pub rounds: Vec<CountRound>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCount {
    pub winners: Vec<uuid::Uuid>,
    pub tied: Vec<uuid::Uuid>,
    pub totals: BTreeMap<uuid::Uuid, usize>,
    pub unresolved_seats: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluralityCount {
    pub winners: Vec<uuid::Uuid>,
    pub totals: BTreeMap<uuid::Uuid, usize>,
}

#[cfg(test)]
fn instant_runoff(ballots: &[Vec<uuid::Uuid>], candidates: &[uuid::Uuid]) -> PresidentialCount {
    instant_runoff_with_decisions(ballots, candidates, &[]).outcome
}

pub fn instant_runoff_with_decisions(
    ballots: &[Vec<uuid::Uuid>],
    candidates: &[uuid::Uuid],
    decisions: &[uuid::Uuid],
) -> RankedCount {
    let mut active: BTreeSet<_> = candidates.iter().copied().collect();
    let mut rounds = Vec::new();
    let mut decisions = decisions.iter();
    loop {
        if active.is_empty() {
            return RankedCount { outcome: PresidentialCount::EliminationTie(Vec::new()), rounds };
        }
        if active.len() == 1 {
            return RankedCount { outcome: PresidentialCount::Winner(*active.first().expect("one active ticket")), rounds };
        }
        let mut totals: BTreeMap<_, usize> = active.iter().copied().map(|id| (id, 0)).collect();
        let mut counted = 0;
        for ballot in ballots {
            if let Some(choice) = ballot.iter().find(|id| active.contains(id)) {
                *totals.get_mut(choice).expect("active ticket has a total") += 1;
                counted += 1;
            }
        }
        let exhausted = ballots.len() - counted;
        if let Some((&candidate, &votes)) = totals.iter().max_by_key(|(_, votes)| *votes)
            && counted > 0 && votes * 2 > counted
        {
            rounds.push(CountRound { number: rounds.len() + 1, totals, exhausted, eliminated: None });
            return RankedCount { outcome: PresidentialCount::Winner(candidate), rounds };
        }
        if active.len() == 2 {
            let high = totals.values().copied().max().unwrap_or(0);
            let tied = totals.iter().filter_map(|(id, votes)| (*votes == high).then_some(*id)).collect();
            rounds.push(CountRound { number: rounds.len() + 1, totals, exhausted, eliminated: None });
            return RankedCount { outcome: PresidentialCount::Runoff(tied), rounds };
        }
        let low = totals.values().copied().min().unwrap_or(0);
        let tied: Vec<_> = totals.iter().filter_map(|(id, votes)| (*votes == low).then_some(*id)).collect();
        let eliminated = if tied.len() == 1 {
            tied[0]
        } else if let Some(choice) = decisions.next().filter(|id| tied.contains(id)) {
            *choice
        } else {
            rounds.push(CountRound { number: rounds.len() + 1, totals, exhausted, eliminated: None });
            return RankedCount { outcome: PresidentialCount::EliminationTie(tied), rounds };
        };
        rounds.push(CountRound { number: rounds.len() + 1, totals, exhausted, eliminated: Some(eliminated) });
        active.remove(&eliminated);
    }
}

pub fn approval_count(ballots: &[Vec<uuid::Uuid>], candidates: &[uuid::Uuid], seats: usize) -> ApprovalCount {
    let mut totals: BTreeMap<_, usize> = candidates.iter().copied().map(|id| (id, 0)).collect();
    for ballot in ballots {
        for candidate in ballot {
            if let Some(total) = totals.get_mut(candidate) { *total += 1; }
        }
    }
    if seats == 0 || candidates.is_empty() {
        return ApprovalCount { winners: Vec::new(), tied: Vec::new(), totals, unresolved_seats: 0 };
    }
    let mut ranked: Vec<_> = totals.iter().map(|(id, votes)| (*id, *votes)).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if ranked.len() <= seats {
        return ApprovalCount { winners: ranked.into_iter().map(|(id, _)| id).collect(), tied: Vec::new(), totals, unresolved_seats: 0 };
    }
    let cutoff = ranked[seats - 1].1;
    let mut winners: Vec<_> = ranked.iter().filter_map(|(id, votes)| (*votes > cutoff).then_some(*id)).collect();
    let tied: Vec<_> = ranked.iter().filter_map(|(id, votes)| (*votes == cutoff).then_some(*id)).collect();
    let remaining = seats - winners.len();
    if tied.len() <= remaining {
        winners.extend(tied);
        ApprovalCount { winners, tied: Vec::new(), totals, unresolved_seats: 0 }
    } else {
        ApprovalCount { winners, tied, totals, unresolved_seats: remaining }
    }
}

pub fn plurality_count(ballots: &[Option<uuid::Uuid>], candidates: &[uuid::Uuid]) -> PluralityCount {
    let mut totals: BTreeMap<_, usize> = candidates.iter().copied().map(|id| (id, 0)).collect();
    for candidate in ballots.iter().flatten() {
        if let Some(total) = totals.get_mut(candidate) { *total += 1; }
    }
    let high = totals.values().copied().max().unwrap_or(0);
    let winners = totals.iter().filter_map(|(id, votes)| (*votes == high).then_some(*id)).collect();
    PluralityCount { winners, totals }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(value: u128) -> uuid::Uuid { uuid::Uuid::from_u128(value) }

    #[test]
    fn presidential_transfers_and_ties() {
        assert_eq!(instant_runoff(&[vec![id(1), id(2)], vec![id(2)], vec![id(2)], vec![id(3)], vec![id(3)]], &[id(1), id(2), id(3)]), PresidentialCount::Winner(id(2)));
        assert_eq!(instant_runoff(&[vec![id(1)], vec![id(2)]], &[id(1), id(2)]), PresidentialCount::Runoff(vec![id(1), id(2)]));
        assert_eq!(instant_runoff(&[], &[id(1)]), PresidentialCount::Winner(id(1)));
    }

    #[test]
    fn presidential_decision_resumes_elimination_tie() {
        assert_eq!(instant_runoff_with_decisions(&[vec![id(1), id(2)], vec![id(2)], vec![id(3)]], &[id(1), id(2), id(3)], &[id(1)]).outcome, PresidentialCount::Winner(id(2)));
    }

    #[test]
    fn council_only_ties_across_the_seat_boundary() {
        let clear = approval_count(&[vec![id(1)], vec![id(1)], vec![id(2)]], &[id(1), id(2), id(3)], 2);
        assert_eq!(clear.winners, vec![id(1), id(2)]);
        assert!(clear.tied.is_empty());
        let tied = approval_count(&[vec![id(1), id(2)], vec![id(1)], vec![id(3)]], &[id(1), id(2), id(3)], 2);
        assert_eq!(tied.winners, vec![id(1)]);
        assert_eq!(tied.tied, vec![id(2), id(3)]);
        assert_eq!(tied.unresolved_seats, 1);

        let exact_fit = approval_count(&[vec![id(1)]], &[id(1), id(2)], 2);
        assert_eq!(exact_fit.winners, vec![id(1), id(2)]);
        assert!(exact_fit.tied.is_empty());

        let zero_vote_boundary = approval_count(&[], &[id(1), id(2), id(3)], 2);
        assert!(zero_vote_boundary.winners.is_empty());
        assert_eq!(zero_vote_boundary.tied, vec![id(1), id(2), id(3)]);
        assert_eq!(zero_vote_boundary.unresolved_seats, 2);
    }

    #[test]
    fn plurality_ignores_ties_among_losers() {
        let count = plurality_count(&[Some(id(1)), Some(id(1)), Some(id(1)), Some(id(1)), Some(id(2)), Some(id(2)), Some(id(3)), Some(id(3)), None], &[id(1), id(2), id(3)]);
        assert_eq!(count.winners, vec![id(1)]);
        assert_eq!(count.totals[&id(1)], 4);

        let top_tie = plurality_count(&[Some(id(1)), Some(id(2)), None], &[id(1), id(2), id(3)]);
        assert_eq!(top_tie.winners, vec![id(1), id(2)]);
        let zero_votes = plurality_count(&[None], &[id(1), id(2)]);
        assert_eq!(zero_votes.winners, vec![id(1), id(2)]);
    }
}
