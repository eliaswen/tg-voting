BEGIN;

ALTER TABLE ballots
    ADD COLUMN submitted_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP;

UPDATE ballots
SET submitted_at = clock_timestamp() - INTERVAL '120 minutes',
    delay = INTERVAL '10 minutes' + random() * INTERVAL '110 minutes';

ALTER TABLE ballots
    ALTER COLUMN delay SET DEFAULT
        (INTERVAL '10 minutes' + random() * INTERVAL '110 minutes'),
    DROP CONSTRAINT ballots_delay_check,
    ADD CONSTRAINT ballots_delay_check
        CHECK (delay >= INTERVAL '10 minutes' AND delay <= INTERVAL '120 minutes');

CREATE INDEX ballots_live_count_idx ON ballots (election_id, revote_id, submitted_at);

COMMIT;
