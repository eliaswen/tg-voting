BEGIN;

CREATE TYPE election_type AS ENUM ('general', 'special');

ALTER TABLE elections
    ADD COLUMN election_type election_type NOT NULL DEFAULT 'general',
    ADD COLUMN published_at TIMESTAMPTZ,
    ADD COLUMN paused_at TIMESTAMPTZ,
    ADD COLUMN paused_stage TEXT,
    ADD COLUMN expected_resume_at TIMESTAMPTZ,
    ADD COLUMN eligibility_snapshotted_at TIMESTAMPTZ;

ALTER TABLE elections
    DROP COLUMN voter_code_registration_starts_at,
    DROP COLUMN voter_code_registration_ends_at;

CREATE TABLE election_positions (
    election_id UUID NOT NULL REFERENCES elections(uuid) ON DELETE CASCADE,
    position candidate_position NOT NULL,
    PRIMARY KEY (election_id, position)
);

INSERT INTO election_positions (election_id, position)
SELECT uuid, position FROM elections CROSS JOIN (VALUES
    ('president'::candidate_position),
    ('vice_president'::candidate_position),
    ('council'::candidate_position)
) AS positions(position);

DROP INDEX candidates_active_election_citizen_unique;

CREATE UNIQUE INDEX candidates_active_election_citizen_group_unique
ON candidates (
    election_id,
    citizen_id,
    (CASE WHEN position IN ('president', 'vice_president', 'council', 'ombudsman') THEN 1 ELSE 2 END)
)
WHERE status = 'active';

CREATE TABLE election_eligibility (
    election_id UUID NOT NULL REFERENCES elections(uuid) ON DELETE CASCADE,
    citizen_id UUID NOT NULL REFERENCES citizens(uuid) ON DELETE RESTRICT,
    credential_issued BOOLEAN NOT NULL DEFAULT FALSE,
    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (election_id, citizen_id)
);

DROP TABLE election_voters;

ALTER TABLE ballots
    ADD COLUMN voting_code_uuid UUID UNIQUE REFERENCES voting_codes(uuid) ON DELETE RESTRICT,
    ADD COLUMN receipt_number TEXT UNIQUE;

ALTER TABLE voting_codes ADD COLUMN code_lookup_hash BYTEA UNIQUE;

COMMIT;
