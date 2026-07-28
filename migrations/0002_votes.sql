BEGIN;


CREATE TYPE candidate_position AS ENUM (
    'president',
    'vice_president',
    'council'
);

CREATE TYPE election_status AS ENUM (
    'draft',
    'registration',
    'voting',
    'closed',
    'certified'
);


CREATE TABLE elections (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    season INTEGER NOT NULL UNIQUE,
    name TEXT NOT NULL,

    status election_status NOT NULL DEFAULT 'draft',

    registration_starts_at TIMESTAMPTZ,
    registration_ends_at TIMESTAMPTZ,

    voting_starts_at TIMESTAMPTZ,
    voting_ends_at TIMESTAMPTZ,

    maximum_council_choices INTEGER NOT NULL DEFAULT 10,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (season > 0),

    CHECK (maximum_council_choices > 0),

    CHECK (
        registration_starts_at IS NULL
        OR registration_ends_at IS NULL
        OR registration_starts_at < registration_ends_at
    ),

    CHECK (
        voting_starts_at IS NULL
        OR voting_ends_at IS NULL
        OR voting_starts_at < voting_ends_at
    )
);


--

CREATE TABLE election_voters (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE CASCADE,

    citizen_id BIGINT NOT NULL
        REFERENCES citizens(id)
        ON DELETE RESTRICT,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (election_id, citizen_id)
);


CREATE TABLE voting_codes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE CASCADE,

    code_hash TEXT NOT NULL,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    used_at TIMESTAMPTZ,

    UNIQUE (election_id, code_hash)
);




CREATE TABLE candidates (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE CASCADE,

    citizen_id BIGINT NOT NULL
        REFERENCES citizens(id)
        ON DELETE RESTRICT,

    position candidate_position NOT NULL,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (election_id, citizen_id, position)
);


--

CREATE TABLE presidential_tickets (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE CASCADE,

    president_candidate_id BIGINT NOT NULL
        REFERENCES candidates(id)
        ON DELETE RESTRICT,

    vice_president_candidate_id BIGINT NOT NULL
        REFERENCES candidates(id)
        ON DELETE RESTRICT,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (
        election_id,
        president_candidate_id,
        vice_president_candidate_id
    ),

    CHECK (
        president_candidate_id <> vice_president_candidate_id
    )
);




CREATE TABLE ballots (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE RESTRICT,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);


CREATE TABLE presidential_votes (
    ballot_id BIGINT PRIMARY KEY
        REFERENCES ballots(id)
        ON DELETE CASCADE,

    ticket_id BIGINT NOT NULL
        REFERENCES presidential_tickets(id)
        ON DELETE RESTRICT
);




CREATE TABLE council_votes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    ballot_id BIGINT NOT NULL
        REFERENCES ballots(id)
        ON DELETE CASCADE,

    candidate_id BIGINT NOT NULL
        REFERENCES candidates(id)
        ON DELETE RESTRICT,

    ranking INTEGER NOT NULL,

    UNIQUE (ballot_id, candidate_id),
    UNIQUE (ballot_id, ranking),

    CHECK (ranking BETWEEN 1 AND 11)
);



CREATE FUNCTION set_database_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.database_updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;




CREATE TRIGGER update_elections_updated_at
BEFORE UPDATE ON elections
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();

CREATE TRIGGER update_candidates_updated_at
BEFORE UPDATE ON candidates
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();

CREATE TRIGGER update_presidential_tickets_updated_at
BEFORE UPDATE ON presidential_tickets
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();



CREATE INDEX voting_codes_unused_by_election_idx
ON voting_codes (election_id)
WHERE used_at IS NULL;

CREATE INDEX candidates_election_position_idx
ON candidates (election_id, position);

CREATE INDEX presidential_tickets_election_idx
ON presidential_tickets (election_id);

CREATE INDEX ballots_election_idx
ON ballots (election_id);

CREATE INDEX presidential_votes_ticket_idx
ON presidential_votes (ticket_id);

CREATE INDEX council_votes_candidate_idx
ON council_votes (candidate_id);

CREATE INDEX council_votes_ballot_ranking_idx
ON council_votes (ballot_id, ranking);


COMMIT;