CREATE TABLE users (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE
);
CREATE TABLE pets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    species TEXT NOT NULL,
    biography TEXT NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users(id),
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
