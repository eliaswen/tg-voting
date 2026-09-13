BEGIN;

ALTER TABLE presidential_votes DROP CONSTRAINT presidential_votes_pkey;
ALTER TABLE presidential_votes ADD COLUMN ranking INTEGER;
UPDATE presidential_votes SET ranking = 1;
ALTER TABLE presidential_votes ALTER COLUMN ranking SET NOT NULL;
ALTER TABLE presidential_votes
    ADD PRIMARY KEY (ballot_uuid, ticket_id),
    ADD UNIQUE (ballot_uuid, ranking),
    ADD CHECK (ranking > 0);

COMMIT;
