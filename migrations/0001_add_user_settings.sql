CREATE TABLE user_setting (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),

    user_uuid UUID NOT NULL
        REFERENCES citizens(uuid)
        ON DELETE CASCADE,

    setting_key TEXT NOT NULL,
    setting_value TEXT NOT NULL,

    user_writable BOOLEAN NOT NULL DEFAULT TRUE,
    user_readable BOOLEAN NOT NULL DEFAULT TRUE,

    database_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    database_updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    last_updated_by_user_uuid UUID
        REFERENCES citizens(uuid)
        ON DELETE SET NULL,

    UNIQUE (user_uuid, setting_key)
);

CREATE TRIGGER update_user_setting_updated_at
BEFORE UPDATE ON user_setting
FOR EACH ROW
EXECUTE FUNCTION set_database_updated_at();