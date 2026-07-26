CREATE TABLE citizens (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    reddit_username TEXT UNIQUE,
    reddit_id TEXT UNIQUE,

    discord_username TEXT UNIQUE,
    discord_id TEXT UNIQUE,

    citizen_id TEXT UNIQUE,

    banned BOOLEAN NOT NULL DEFAULT FALSE,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (
        reddit_username IS NOT NULL
        OR discord_username IS NOT NULL
    ),

    CHECK (
        reddit_id IS NULL
        OR reddit_username IS NOT NULL
    ),

    CHECK (
        discord_id IS NULL
        OR discord_username IS NOT NULL
    )
);

CREATE FUNCTION set_citizens_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.database_updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER update_citizens_updated_at
BEFORE UPDATE ON citizens
FOR EACH ROW
EXECUTE FUNCTION set_citizens_updated_at();