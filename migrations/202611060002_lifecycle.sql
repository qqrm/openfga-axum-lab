ALTER TABLE pets DROP CONSTRAINT pets_status_check;
ALTER TABLE pets ADD CONSTRAINT pets_status_check
    CHECK (status IN ('pending', 'active', 'deleting', 'deleted'));
ALTER TABLE pets ADD COLUMN delete_actor TEXT REFERENCES users(id);
ALTER TABLE pets ADD COLUMN idempotency_key TEXT;
ALTER TABLE pets ADD COLUMN request_hash BYTEA;
CREATE UNIQUE INDEX pets_owner_idempotency
    ON pets (owner_id, idempotency_key) WHERE idempotency_key IS NOT NULL;
ALTER TABLE pets ADD CONSTRAINT pets_request_identity_check
    CHECK ((idempotency_key IS NULL) = (request_hash IS NULL));
