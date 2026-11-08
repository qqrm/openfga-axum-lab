-- Independent source/target binding for the Openlane organization/task model.
-- Historical Lakekeeper migrations and their journal remain unchanged.
CREATE TABLE openlane_authorization_migration (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    source_contract TEXT NOT NULL DEFAULT 'openlane-organization-task-v1'
        CHECK (source_contract = 'openlane-organization-task-v1'),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'writing', 'verifying', 'active')),
    store_id TEXT,
    model_id TEXT,
    model_sha256 TEXT,
    source_sha256 TEXT,
    source_json TEXT,
    next_tuple BIGINT NOT NULL DEFAULT 0 CHECK (next_tuple >= 0),
    tuple_count BIGINT NOT NULL DEFAULT 0 CHECK (tuple_count >= 0),
    verified_checks BIGINT NOT NULL DEFAULT 0 CHECK (verified_checks >= 0),
    attempted BOOLEAN NOT NULL DEFAULT false,
    activated_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (next_tuple <= tuple_count),
    CHECK ((status = 'pending') = (source_sha256 IS NULL)),
    CHECK ((status = 'pending') = (source_json IS NULL)),
    CHECK (status = 'pending' OR
        (store_id IS NOT NULL AND model_id IS NOT NULL AND model_sha256 IS NOT NULL)),
    CHECK ((status = 'active') = (activated_at IS NOT NULL)),
    CHECK (status <> 'active' OR NOT attempted)
);
INSERT INTO openlane_authorization_migration DEFAULT VALUES;

-- Freeze the authorization source across crashes and restarts while the bound
-- projection is incomplete. The canonical snapshot remains in the journal
-- after cutover, when the application may legitimately change live SQL rows.
CREATE FUNCTION guard_openlane_authorization_source() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE migration_status TEXT;
BEGIN
    SELECT status INTO migration_status FROM openlane_authorization_migration
        WHERE singleton FOR SHARE;
    IF migration_status IN ('writing', 'verifying') THEN
        RAISE EXCEPTION 'Openlane source is frozen while authorization migration is incomplete';
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER freeze_openlane_users BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON users FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
CREATE TRIGGER freeze_openlane_organizations BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON organizations FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
CREATE TRIGGER freeze_openlane_members BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON organization_members FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
CREATE TRIGGER freeze_openlane_permissions BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON organization_permissions FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
CREATE TRIGGER freeze_openlane_tasks BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON tasks FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
CREATE TRIGGER freeze_openlane_relations BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON task_relations FOR EACH STATEMENT EXECUTE FUNCTION guard_openlane_authorization_source();
