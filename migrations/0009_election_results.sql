BEGIN;

CREATE TABLE election_results (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    election_id UUID NOT NULL REFERENCES elections(uuid) ON DELETE CASCADE,
    contest candidate_position NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('provisional', 'review', 'runoff', 'certified', 'voided')),
    calculated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    reviewed_at TIMESTAMPTZ,
    reviewed_by UUID REFERENCES citizens(uuid),
    review_reason TEXT,
    certified_at TIMESTAMPTZ,
    certified_by UUID REFERENCES citizens(uuid),
    certification_reason TEXT,
    UNIQUE (election_id, contest)
);

CREATE TABLE election_result_candidates (
    result_id UUID NOT NULL REFERENCES election_results(uuid) ON DELETE CASCADE,
    candidate_id UUID NOT NULL REFERENCES candidates(uuid) ON DELETE RESTRICT,
    votes INTEGER NOT NULL DEFAULT 0 CHECK (votes >= 0),
    elected BOOLEAN NOT NULL DEFAULT FALSE,
    tied BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (result_id, candidate_id)
);

CREATE TABLE election_result_tickets (
    result_id UUID NOT NULL REFERENCES election_results(uuid) ON DELETE CASCADE,
    ticket_id UUID NOT NULL REFERENCES presidential_tickets(uuid) ON DELETE RESTRICT,
    votes INTEGER NOT NULL DEFAULT 0 CHECK (votes >= 0),
    elected BOOLEAN NOT NULL DEFAULT FALSE,
    tied BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (result_id, ticket_id)
);

CREATE TABLE election_result_actions (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    result_id UUID NOT NULL REFERENCES election_results(uuid) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES citizens(uuid),
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);
COMMIT;
