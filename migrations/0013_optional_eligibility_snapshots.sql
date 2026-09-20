BEGIN;

ALTER TABLE elections
    ADD COLUMN eligibility_snapshot_enabled BOOLEAN NOT NULL DEFAULT TRUE;

COMMIT;
