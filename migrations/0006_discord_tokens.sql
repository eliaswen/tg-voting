ALTER TABLE sessions
ADD COLUMN discord_access_token TEXT,
ADD COLUMN discord_refresh_token TEXT,
ADD COLUMN discord_token_expires_at TIMESTAMPTZ;

CREATE INDEX sessions_discord_token_expires_at_idx
    ON sessions (discord_token_expires_at)
    WHERE revoked_at IS NULL;
