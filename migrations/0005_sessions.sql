CREATE TABLE sessions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    associated_citizen UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    auth_code_hash BYTEA NOT NULL UNIQUE,

    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,

    device_type TEXT,
    device_name TEXT,

    CHECK (expires_at > created_at)
);

CREATE INDEX sessions_associated_citizen_idx
    ON sessions (associated_citizen);

CREATE INDEX sessions_expires_at_idx
    ON sessions (expires_at);