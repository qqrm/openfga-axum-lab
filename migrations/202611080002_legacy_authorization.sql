-- Migration input is a documented relational source contract for the lab, not
-- a claim to be Lakekeeper's internal PostgreSQL schema. Resource owners and
-- hierarchy come from catalog_*; these tables retain existing ACL/role rows.
CREATE TABLE legacy_project_grants (
    project_id text NOT NULL REFERENCES catalog_projects(id),
    subject_kind text NOT NULL,
    subject_id text NOT NULL,
    relation text NOT NULL,
    PRIMARY KEY (project_id, subject_kind, subject_id, relation)
);
CREATE TABLE legacy_resource_grants (
    object_kind text NOT NULL,
    object_id text NOT NULL,
    subject_kind text NOT NULL,
    subject_id text NOT NULL,
    relation text NOT NULL,
    PRIMARY KEY (object_kind, object_id, subject_kind, subject_id, relation)
);
CREATE TABLE legacy_role_assignments (
    role_id text NOT NULL REFERENCES catalog_roles(id),
    subject_kind text NOT NULL,
    subject_id text NOT NULL,
    PRIMARY KEY (role_id, subject_kind, subject_id)
);

CREATE TABLE authorization_migration (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'writing', 'verifying', 'active')),
    store_id text,
    model_id text,
    model_sha256 text,
    source_sha256 text,
    next_tuple bigint NOT NULL DEFAULT 0 CHECK (next_tuple >= 0),
    tuple_count bigint NOT NULL DEFAULT 0 CHECK (tuple_count >= 0),
    verified_checks bigint NOT NULL DEFAULT 0 CHECK (verified_checks >= 0),
    activated_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (next_tuple <= tuple_count),
    CHECK ((status = 'pending') = (source_sha256 IS NULL)),
    CHECK (status = 'pending' OR
        (store_id IS NOT NULL AND model_id IS NOT NULL AND model_sha256 IS NOT NULL)),
    CHECK ((status = 'active') = (activated_at IS NOT NULL))
);
INSERT INTO authorization_migration DEFAULT VALUES;

-- The frozen legacy tables are an audit source after activation. They must not
-- be replayed after a live revoke or an ownership transfer. To migrate a new
-- source/target pair, an operator needs an explicit new rollout procedure.
CREATE FUNCTION guard_legacy_authorization_source() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE bound_hash text;
BEGIN
    SELECT source_sha256 INTO bound_hash FROM authorization_migration WHERE singleton FOR SHARE;
    IF bound_hash IS NOT NULL THEN
        RAISE EXCEPTION 'authorization source is frozen by a bound migration';
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER freeze_legacy_project_grants BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON legacy_project_grants FOR EACH STATEMENT EXECUTE FUNCTION guard_legacy_authorization_source();
CREATE TRIGGER freeze_legacy_resource_grants BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON legacy_resource_grants FOR EACH STATEMENT EXECUTE FUNCTION guard_legacy_authorization_source();
CREATE TRIGGER freeze_legacy_role_assignments BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON legacy_role_assignments FOR EACH STATEMENT EXECUTE FUNCTION guard_legacy_authorization_source();

-- Application rows remain mutable once the new authorization graph is active.
-- During migration the source is stable, including across process restarts.
CREATE FUNCTION guard_catalog_during_authorization_migration() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE migration_status text;
BEGIN
    SELECT status INTO migration_status FROM authorization_migration WHERE singleton FOR SHARE;
    IF migration_status IN ('writing', 'verifying') THEN
        RAISE EXCEPTION 'catalog source is frozen while authorization migration is incomplete';
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER freeze_catalog_projects BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_projects FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
CREATE TRIGGER freeze_catalog_memberships BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_memberships FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
CREATE TRIGGER freeze_catalog_warehouses BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_warehouses FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
CREATE TRIGGER freeze_catalog_namespaces BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_namespaces FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
CREATE TRIGGER freeze_catalog_tables BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_tables FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
CREATE TRIGGER freeze_catalog_roles BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON catalog_roles FOR EACH STATEMENT EXECUTE FUNCTION guard_catalog_during_authorization_migration();
