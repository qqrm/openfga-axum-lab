-- Add the missing pre-send marker without rewriting any applied migration.
-- Remote requests can outlive the migration process. A replay that sees this
-- marker must retain independent authz_cache_state debt even after activation.
ALTER TABLE authorization_migration
    ADD COLUMN attempted BOOLEAN NOT NULL DEFAULT false;

-- Older unfinished migrations have no reliable attempt history, including a
-- verifying checkpoint whose final batch was already acknowledged. Record the
-- uncertainty globally rather than inventing an unacknowledged batch beyond
-- the persisted tuple_count. Replaying or activating must not clear this debt.
UPDATE authz_cache_state SET pending = pending + 1, generation = generation + 1
    WHERE singleton AND EXISTS (
        SELECT 1 FROM authorization_migration
        WHERE status IN ('writing', 'verifying')
    );

ALTER TABLE authorization_migration
    ADD CONSTRAINT catalog_migration_active_acknowledged
    CHECK (status <> 'active' OR NOT attempted);
