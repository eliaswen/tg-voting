use chrono::{DateTime, Utc};

pub const DIRECT_POSITIONS: [(&str, &str); 6] = [
    ("president", "President"),
    ("council", "Council"),
    ("ombudsman", "Ombudsman"),
    ("moderator", "Moderator"),
    ("moderator_placeholder_1", "moderator placeholder 1"),
    ("moderator_placeholder_2", "moderator placeholder 2"),
];

pub fn position_label(position: &str) -> &'static str {
    DIRECT_POSITIONS
        .iter()
        .find(|(value, _)| *value == position)
        .map(|(_, label)| *label)
        .unwrap_or("Unknown position")
}

pub fn position_group(position: &str) -> Option<u8> {
    match position {
        "president" | "vice_president" | "council" | "ombudsman" => Some(1),
        "moderator" | "moderator_placeholder_1" | "moderator_placeholder_2" => Some(2),
        _ => None,
    }
}

pub struct Timeline {
    pub stage: String,
    pub stage_label: String,
    pub next: String,
}

pub fn timeline(
    stored_status: &str,
    registration_starts_at: Option<DateTime<Utc>>,
    registration_ends_at: Option<DateTime<Utc>>,
    voting_starts_at: Option<DateTime<Utc>>,
    voting_ends_at: Option<DateTime<Utc>>,
    paused_stage: Option<&str>,
    now: DateTime<Utc>,
) -> Timeline {
    if stored_status == "draft" {
        return fixed(
            "draft",
            "Draft",
            "The election will be published as upcoming.",
        );
    }
    if stored_status == "paused" {
        return fixed(
            "paused",
            "Paused",
            &format!(
                "The election will resume from {}.",
                paused_stage.unwrap_or("its previous stage")
            ),
        );
    }
    if matches!(stored_status, "canceled" | "closed" | "certified") {
        let next = match stored_status {
            "closed" => "The election can be certified.",
            "certified" => "The election is complete.",
            _ => "The election will not continue.",
        };
        return fixed(stored_status, &title(stored_status), next);
    }
    if let (
        Some(registration_start),
        Some(registration_end),
        Some(voting_start),
        Some(voting_end),
    ) = (
        registration_starts_at,
        registration_ends_at,
        voting_starts_at,
        voting_ends_at,
    ) {
        if now < registration_start {
            return scheduled(
                "upcoming",
                "Upcoming",
                "Candidate registration opens",
                registration_start,
            );
        }
        if now < registration_end {
            return scheduled(
                "registration",
                "Registration",
                "Candidate registration closes",
                registration_end,
            );
        }
        if now < voting_start {
            return scheduled(
                "upcoming",
                "Upcoming",
                "Registration ended, voting opens",
                voting_start,
            );
        }
        if now < voting_end {
            return scheduled(
                "voting",
                "Voting",
                "Voting closes and counting begins",
                voting_end,
            );
        }
        return fixed(
            "counting",
            "Counting",
            "Results announced and election closed",
        );
    }
    fixed(
        "unknown",
        "Unknown",
        "The election's current stage could not be determined.",
    )
}

fn scheduled(stage: &str, label: &str, next: &str, next_at: DateTime<Utc>) -> Timeline {
    let _ = next_at;
    Timeline {
        stage: stage.to_string(),
        stage_label: label.to_string(),
        next: next.to_string(),
    }
}

fn fixed(stage: &str, label: &str, next: &str) -> Timeline {
    Timeline {
        stage: stage.to_string(),
        stage_label: label.to_string(),
        next: next.to_string(),
    }
}

fn title(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_groups_are_separate() {
        assert_eq!(position_group("president"), Some(1));
        assert_eq!(position_group("ombudsman"), Some(1));
        assert_eq!(position_group("moderator"), Some(2));
    }
}
