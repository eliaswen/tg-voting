BEGIN;

CREATE TABLE discord_oauth_requests (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    citizen_uuid UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    state_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (expires_at > database_created_at)
);


CREATE INDEX discord_oauth_requests_active_idx
ON discord_oauth_requests (expires_at)
WHERE used_at IS NULL;

CREATE TYPE census_status AS ENUM (
    'filled_out',
    'ineligible',
    'incorrect',
    'not_filled_out',
    'other',
    'to_be_set'
);


CREATE TABLE censuses (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    census_month DATE NOT NULL UNIQUE,
    active BOOLEAN NOT NULL DEFAULT FALSE,

    created_by_citizen_uuid UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE RESTRICT,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    activated_at TIMESTAMPTZ,

    CHECK (date_trunc('month', census_month)::date = census_month),
    CHECK ((active AND activated_at IS NOT NULL) OR NOT active)
);


CREATE UNIQUE INDEX censuses_one_active_idx
ON censuses (active)
WHERE active = TRUE;


CREATE TABLE census_entries (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    census_uuid UUID NOT NULL
        REFERENCES censuses(uuid)
        ON DELETE CASCADE,

    citizen_uuid UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE RESTRICT,

    status census_status NOT NULL DEFAULT 'to_be_set',

    last_updated_by_citizen_uuid UUID
        REFERENCES citizens(uuid)
        ON DELETE SET NULL,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    UNIQUE (census_uuid, citizen_uuid)
);


CREATE INDEX census_entries_citizen_idx
ON census_entries (citizen_uuid);


CREATE TRIGGER update_census_entries_updated_at
BEFORE UPDATE ON census_entries
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();


COMMIT;
