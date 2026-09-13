BEGIN;

ALTER TABLE candidates
    ADD CONSTRAINT candidates_uuid_position_unique UNIQUE (uuid, position);

CREATE TABLE candidate_votes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ballot_uuid UUID NOT NULL REFERENCES ballots(uuid) ON DELETE CASCADE,
    candidate_id UUID NOT NULL,
    position candidate_position NOT NULL,
    ranking INTEGER NOT NULL,
    FOREIGN KEY (candidate_id, position) REFERENCES candidates(uuid, position) ON DELETE RESTRICT,
    UNIQUE (ballot_uuid, candidate_id),
    CHECK (ranking > 0),
    CHECK (position <> 'council' OR ranking = 1)
);
CREATE INDEX candidate_votes_candidate_idx ON candidate_votes (candidate_id);
CREATE UNIQUE INDEX candidate_votes_single_choice_idx
    ON candidate_votes (ballot_uuid, position)
    WHERE position <> 'council';

INSERT INTO candidate_votes (ballot_uuid, candidate_id, position, ranking)
SELECT ballot_uuid, candidate_id, 'council', 1
FROM council_votes
ON CONFLICT (ballot_uuid, candidate_id) DO NOTHING;

DROP TABLE council_votes;

COMMIT;
