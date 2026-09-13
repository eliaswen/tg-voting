BEGIN;

CREATE TYPE election_revote_mode AS ENUM ('all_contests', 'selected_contest', 'runoff', 'full_restart');

CREATE TABLE election_revotes (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    election_id UUID NOT NULL REFERENCES elections(uuid) ON DELETE CASCADE,
    contest candidate_position,
    mode election_revote_mode NOT NULL,
    reason TEXT NOT NULL CHECK (char_length(trim(reason)) > 0),
    requested_by UUID NOT NULL REFERENCES citizens(uuid),
    source_result_id UUID REFERENCES election_results(uuid) ON DELETE CASCADE,
    unresolved_seats INTEGER CHECK (unresolved_seats > 0),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    registration_starts_at TIMESTAMPTZ,
    registration_ends_at TIMESTAMPTZ,
    voting_starts_at TIMESTAMPTZ,
    voting_ends_at TIMESTAMPTZ,
    CHECK ((mode IN ('selected_contest', 'runoff')) = (contest IS NOT NULL)),
    CHECK ((mode NOT IN ('selected_contest', 'runoff')) = (contest IS NULL)),
    CHECK ((mode = 'runoff') = (source_result_id IS NOT NULL)),
    CHECK (unresolved_seats IS NULL OR (mode = 'runoff' AND contest = 'council'))
);

CREATE TABLE election_revote_candidates (
    revote_id UUID NOT NULL REFERENCES election_revotes(uuid) ON DELETE CASCADE,
    contest candidate_position NOT NULL,
    candidate_id UUID NOT NULL REFERENCES candidates(uuid) ON DELETE RESTRICT,
    PRIMARY KEY (revote_id, contest, candidate_id)
);

CREATE TABLE election_revote_tickets (
    revote_id UUID NOT NULL REFERENCES election_revotes(uuid) ON DELETE CASCADE,
    ticket_id UUID NOT NULL REFERENCES presidential_tickets(uuid) ON DELETE RESTRICT,
    PRIMARY KEY (revote_id, ticket_id)
);

ALTER TABLE elections
    ADD COLUMN active_revote_id UUID REFERENCES election_revotes(uuid),
    ADD COLUMN voter_code_registration_starts_at TIMESTAMPTZ,
    ADD COLUMN voter_code_registration_ends_at TIMESTAMPTZ;

ALTER TABLE ballots ADD COLUMN revote_id UUID REFERENCES election_revotes(uuid);
ALTER TABLE voting_codes ADD COLUMN revote_id UUID REFERENCES election_revotes(uuid);

ALTER TABLE ballots DROP CONSTRAINT ballots_election_id_authorization_hash_key;
ALTER TABLE ballots ADD CONSTRAINT ballots_round_authorization_unique
    UNIQUE NULLS NOT DISTINCT (election_id, revote_id, authorization_hash);

ALTER TABLE voting_codes DROP CONSTRAINT voting_codes_election_id_code_hash_key;
ALTER TABLE voting_codes ADD CONSTRAINT voting_codes_round_hash_unique
    UNIQUE NULLS NOT DISTINCT (election_id, revote_id, code_hash);

ALTER TABLE election_results DROP CONSTRAINT election_results_election_id_contest_key;
ALTER TABLE election_results
    ADD COLUMN revote_id UUID REFERENCES election_revotes(uuid),
    ADD COLUMN round_number INTEGER NOT NULL DEFAULT 1 CHECK (round_number > 0),
    ADD COLUMN count_details JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE election_results ADD CONSTRAINT election_results_round_unique
    UNIQUE NULLS NOT DISTINCT (election_id, contest, revote_id);

ALTER TABLE election_result_actions
    ADD COLUMN candidate_id UUID REFERENCES candidates(uuid) ON DELETE SET NULL,
    ADD COLUMN ticket_id UUID REFERENCES presidential_tickets(uuid) ON DELETE SET NULL,
    ADD CONSTRAINT election_result_actions_single_target
        CHECK (num_nonnulls(candidate_id, ticket_id) <= 1);

CREATE INDEX ballots_round_idx ON ballots (election_id, revote_id);
CREATE INDEX voting_codes_round_unused_idx ON voting_codes (election_id, revote_id) WHERE used = FALSE;
CREATE INDEX election_results_current_idx ON election_results (election_id, revote_id, contest);

COMMIT;
