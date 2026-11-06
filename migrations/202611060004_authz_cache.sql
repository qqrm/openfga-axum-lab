CREATE TABLE authz_cache_state (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    generation bigint NOT NULL DEFAULT 0 CHECK (generation >= 0),
    pending bigint NOT NULL DEFAULT 0 CHECK (pending >= 0)
);

INSERT INTO authz_cache_state DEFAULT VALUES;
