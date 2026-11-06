ALTER TABLE users DROP COLUMN token_hash;
ALTER TABLE users ADD COLUMN password_hash TEXT;
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_id_idx ON sessions(user_id);
CREATE TABLE login_attempts (
    subject TEXT PRIMARY KEY,
    attempts INTEGER NOT NULL,
    window_start TIMESTAMPTZ NOT NULL
);
