BEGIN;

CREATE TABLE pending_oauth_logins (
    request_id UUID PRIMARY KEY,
    flow TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    session_token TEXT,
    error_code TEXT,
    device_type TEXT NOT NULL,
    device_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CHECK (flow IN ('browser', 'device')),
    CHECK (status IN ('pending', 'processing', 'complete', 'failed')),
    CHECK (expires_at > created_at),
    CHECK (
        (status IN ('pending', 'processing') AND session_token IS NULL AND error_code IS NULL)
        OR (status = 'complete' AND session_token IS NOT NULL AND error_code IS NULL)
        OR (status = 'failed' AND session_token IS NULL AND error_code IS NOT NULL)
    )
);

CREATE INDEX pending_oauth_logins_expires_at_idx
ON pending_oauth_logins (expires_at);

CREATE FUNCTION notify_pending_oauth_login_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify(
        'oauth_login_updates',
        COALESCE(NEW.request_id, OLD.request_id)::text
    );
    RETURN COALESCE(NEW, OLD);
END;
$$;

CREATE TRIGGER pending_oauth_login_update
AFTER INSERT OR UPDATE OR DELETE ON pending_oauth_logins
FOR EACH ROW
EXECUTE FUNCTION notify_pending_oauth_login_update();

COMMIT;
