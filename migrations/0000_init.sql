BEGIN;

CREATE EXTENSION IF NOT EXISTS pgcrypto;


-- ============================================================
-- TYPES
-- ============================================================

CREATE TYPE candidate_position AS ENUM (
    'president',
    'vice_president',
    'council'
);


CREATE TYPE election_status AS ENUM (
    'draft',
    'registration',
    'voting',
    'paused',
    'closed',
    'canceled',
    'certified'
);


CREATE TYPE registration_status AS ENUM (
    'active',
    'withdrawn',
    'invalidated'
);


-- ============================================================
-- UPDATED-AT TRIGGER
-- ============================================================

CREATE FUNCTION set_database_updated_at()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.database_updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$;


-- ============================================================
-- CITIZENS
-- ============================================================

CREATE TABLE citizens (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    citizen_id TEXT UNIQUE,

    banned BOOLEAN NOT NULL DEFAULT FALSE,

    -- Permission bit mask:
    -- bit 0  = citizen
    -- bit 1  = minister
    -- bit 2  = census minister
    -- bit 3  = election minister
    -- bit 4  = admin
    -- bit 5  = superadmin
    -- bit 6+ = reserved
    role BIGINT NOT NULL DEFAULT 0,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (role >= 0)
);


CREATE TRIGGER update_citizens_updated_at
BEFORE UPDATE ON citizens
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- AUTHENTIK IDENTITIES
-- ============================================================
--
-- issuer  = OIDC "iss" claim
-- subject = OIDC "sub" claim
CREATE TABLE authentik_identities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,

    preferred_username TEXT,
    email TEXT,
    display_name TEXT,

    last_authenticated_at TIMESTAMPTZ,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (issuer, subject),
    UNIQUE (citizen_id)
);


CREATE TRIGGER update_authentik_identities_updated_at
BEFORE UPDATE ON authentik_identities
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- DISCORD LINKS
-- ============================================================
CREATE TABLE citizen_discord_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    discord_user_id TEXT NOT NULL,
    discord_username TEXT,
    discord_display_name TEXT,

    verified_at TIMESTAMPTZ,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (citizen_id),
    UNIQUE (discord_user_id)
);


CREATE TRIGGER update_citizen_discord_links_updated_at
BEFORE UPDATE ON citizen_discord_links
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- REDDIT LINKS
-- ============================================================

-- No automatic verification for Reddit
CREATE TABLE citizen_reddit_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    reddit_username TEXT NOT NULL,

    verified_at TIMESTAMPTZ,
    verification_method TEXT,

    verified_by_citizen_id UUID
        REFERENCES citizens(uuid)
        ON DELETE SET NULL,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (citizen_id)
);


-- Reddit usernames are treated as case-insensitive.
CREATE UNIQUE INDEX citizen_reddit_links_username_unique
ON citizen_reddit_links (lower(reddit_username));


CREATE TRIGGER update_citizen_reddit_links_updated_at
BEFORE UPDATE ON citizen_reddit_links
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- ELECTIONS
-- ============================================================

CREATE TABLE elections (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    season INTEGER NOT NULL UNIQUE,
    name TEXT NOT NULL,

    status election_status NOT NULL DEFAULT 'draft',

    registration_starts_at TIMESTAMPTZ,
    registration_ends_at TIMESTAMPTZ,

    voter_code_registration_starts_at TIMESTAMPTZ,
    voter_code_registration_ends_at TIMESTAMPTZ,

    voting_starts_at TIMESTAMPTZ,
    voting_ends_at TIMESTAMPTZ,

    maximum_council_choices INTEGER NOT NULL DEFAULT 10,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (season > 0),
    CHECK (maximum_council_choices > 0)
);


CREATE TRIGGER update_elections_updated_at
BEFORE UPDATE ON elections
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- ELECTION VOTERS
-- ============================================================

-- This table tracks eligibility and whether the citizen has received an
-- anonymous voting credential.
CREATE TABLE election_voters (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    election_id BIGINT NOT NULL
        REFERENCES elections(id)
        ON DELETE CASCADE,

    citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE RESTRICT,

    credential_issued BOOLEAN NOT NULL DEFAULT FALSE,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (election_id, citizen_id)
);


CREATE INDEX election_voters_citizen_idx
ON election_voters (citizen_id);


-- ============================================================
-- ANONYMOUS VOTING CODES
-- ============================================================
CREATE TABLE voting_codes (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    election_id UUID NOT NULL
        REFERENCES elections(uuid)
        ON DELETE CASCADE,

    -- Store an expensive password-style hash of the code.
    -- The application can use Argon2id or another suitable password hash.
    code_hash TEXT NOT NULL,

    used BOOLEAN NOT NULL DEFAULT FALSE,

    UNIQUE (election_id, code_hash)
);


CREATE INDEX voting_codes_unused_by_election_idx
ON voting_codes (election_id)
WHERE used = FALSE;


-- ============================================================
-- CANDIDATES
-- ============================================================

CREATE TABLE candidates (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id UUID NOT NULL
        REFERENCES elections(uuid)
        ON DELETE CASCADE,

    citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE RESTRICT,

    position candidate_position NOT NULL,

    election_display_name TEXT NOT NULL,
    party TEXT NOT NULL,
    status registration_status NOT NULL DEFAULT 'active',

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (char_length(trim(election_display_name)) > 0),
    CHECK (char_length(trim(party)) > 0)
);


CREATE INDEX candidates_election_position_idx
ON candidates (election_id, position);


CREATE INDEX candidates_citizen_idx
ON candidates (citizen_id);


CREATE UNIQUE INDEX candidates_active_election_citizen_unique
ON candidates (election_id, citizen_id)
WHERE status = 'active';


CREATE TRIGGER update_candidates_updated_at
BEFORE UPDATE ON candidates
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- PRESIDENTIAL TICKETS
-- ============================================================
CREATE TABLE presidential_tickets (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id UUID NOT NULL
        REFERENCES elections(uuid)
        ON DELETE CASCADE,

    president_candidate_id UUID NOT NULL
        REFERENCES candidates(uuid)
        ON DELETE RESTRICT,

    vice_president_candidate_id UUID NOT NULL
        REFERENCES candidates(uuid)
        ON DELETE RESTRICT,

    status registration_status NOT NULL DEFAULT 'active',

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


CREATE INDEX presidential_tickets_election_idx
ON presidential_tickets (election_id);


CREATE TRIGGER update_presidential_tickets_updated_at
BEFORE UPDATE ON presidential_tickets
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


-- ============================================================
-- PRESIDENTIAL TICKET MESSAGES
-- ============================================================
CREATE TABLE presidential_ticket_messages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    presidential_ticket_id UUID NOT NULL
        REFERENCES presidential_tickets(uuid)
        ON DELETE CASCADE,

    position INTEGER NOT NULL,
    message TEXT NOT NULL,

    UNIQUE (presidential_ticket_id, position),

    CHECK (position BETWEEN 1 AND 5),
    CHECK (char_length(message) BETWEEN 1 AND 100)
);


-- ============================================================
-- ELECTION CHANGE LOG
-- ============================================================
CREATE TABLE election_change_log (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    election_id UUID NOT NULL
        REFERENCES elections(uuid)
        ON DELETE CASCADE,

    actor_citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE RESTRICT,

    actor_display_name TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_uuid UUID NOT NULL,
    previous_value TEXT NOT NULL,
    new_value TEXT NOT NULL,
    reason TEXT NOT NULL,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (char_length(trim(actor_display_name)) > 0),
    CHECK (char_length(trim(target_type)) > 0),
    CHECK (char_length(trim(reason)) > 0)
);


CREATE INDEX election_change_log_election_idx
ON election_change_log (election_id, database_created_at, id);


-- ============================================================
-- ANONYMOUS BALLOTS
-- ============================================================
CREATE TABLE ballots (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    election_id UUID NOT NULL
        REFERENCES elections(uuid)
        ON DELETE RESTRICT,

    authorization_hash BYTEA NOT NULL,

    delay INTERVAL NOT NULL DEFAULT INTERVAL '0 seconds',

    UNIQUE (election_id, authorization_hash),

    CHECK (delay >= INTERVAL '0 seconds')
);


CREATE INDEX ballots_election_idx
ON ballots (election_id);


-- ============================================================
-- PRESIDENTIAL VOTES
-- ============================================================
CREATE TABLE presidential_votes (
    ballot_uuid UUID PRIMARY KEY
        REFERENCES ballots(uuid)
        ON DELETE CASCADE,

    ticket_id UUID NOT NULL
        REFERENCES presidential_tickets(uuid)
        ON DELETE RESTRICT
);


CREATE INDEX presidential_votes_ticket_idx
ON presidential_votes (ticket_id);


-- ============================================================
-- COUNCIL VOTES
-- ============================================================
CREATE TABLE council_votes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    ballot_uuid UUID NOT NULL
        REFERENCES ballots(uuid)
        ON DELETE CASCADE,

    candidate_id UUID NOT NULL
        REFERENCES candidates(uuid)
        ON DELETE RESTRICT,

    ranking INTEGER NOT NULL,

    UNIQUE (ballot_uuid, candidate_id),
    UNIQUE (ballot_uuid, ranking),

    CHECK (ranking > 0)
);


CREATE INDEX council_votes_candidate_idx
ON council_votes (candidate_id);


-- ============================================================
-- SESSIONS
-- ============================================================

CREATE TABLE sessions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    associated_citizen_id UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    auth_code_hash BYTEA NOT NULL UNIQUE,

    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,

    device_type TEXT,
    device_name TEXT,

    oauth_access_token BYTEA,
    oauth_refresh_token BYTEA,
    oauth_token_expires_at TIMESTAMPTZ,

    CHECK (expires_at > created_at)
);


CREATE INDEX sessions_associated_citizen_idx
ON sessions (associated_citizen_id);


CREATE INDEX sessions_active_expires_at_idx
ON sessions (expires_at)
WHERE revoked_at IS NULL;


CREATE INDEX sessions_oauth_token_expires_at_idx
ON sessions (oauth_token_expires_at)
WHERE revoked_at IS NULL
  AND oauth_token_expires_at IS NOT NULL;


COMMIT;
