BEGIN;

ALTER TABLE elections
    ADD COLUMN council_seats INTEGER;

UPDATE elections SET council_seats = maximum_council_choices;

ALTER TABLE elections
    ALTER COLUMN council_seats SET DEFAULT 10,
    ALTER COLUMN council_seats SET NOT NULL,
    ADD CONSTRAINT elections_council_seats_positive CHECK (council_seats > 0),
    DROP COLUMN maximum_council_choices;

COMMIT;
