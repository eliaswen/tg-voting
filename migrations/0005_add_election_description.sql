BEGIN;

ALTER TABLE elections ADD COLUMN description TEXT;

ALTER TABLE elections
    ALTER COLUMN description SET DEFAULT 'No description is available for this election.';

UPDATE elections
SET description = 'No description is available for this election.'
WHERE description IS NULL OR trim(description) = '';

ALTER TABLE elections
    ALTER COLUMN description SET NOT NULL,
    ADD CONSTRAINT elections_description_not_blank CHECK (char_length(trim(description)) > 0);

COMMIT;
