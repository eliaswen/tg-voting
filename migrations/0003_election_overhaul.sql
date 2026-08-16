ALTER TYPE candidate_position ADD VALUE IF NOT EXISTS 'ombudsman';
ALTER TYPE candidate_position ADD VALUE IF NOT EXISTS 'moderator';
ALTER TYPE candidate_position ADD VALUE IF NOT EXISTS 'moderator_placeholder_1';
ALTER TYPE candidate_position ADD VALUE IF NOT EXISTS 'moderator_placeholder_2';

ALTER TYPE election_status ADD VALUE IF NOT EXISTS 'upcoming';
ALTER TYPE election_status ADD VALUE IF NOT EXISTS 'counting';
