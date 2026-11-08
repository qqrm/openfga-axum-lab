-- Preserve previously applied migrations and their data. The runtime now uses
-- the catalog schema; it never interprets historic demo rows as catalog grants.
CREATE TABLE catalog_projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE catalog_memberships (
    project_id TEXT NOT NULL REFERENCES catalog_projects(id),
    user_id TEXT NOT NULL REFERENCES users(id),
    PRIMARY KEY (project_id, user_id)
);
CREATE TABLE catalog_warehouses (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES catalog_projects(id),
    name TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, owner_id) REFERENCES catalog_memberships(project_id, user_id)
);
CREATE TABLE catalog_namespaces (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    warehouse_id TEXT NOT NULL,
    name TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    UNIQUE (project_id, warehouse_id, id),
    FOREIGN KEY (project_id, warehouse_id) REFERENCES catalog_warehouses(project_id, id),
    FOREIGN KEY (project_id, owner_id) REFERENCES catalog_memberships(project_id, user_id)
);
CREATE TABLE catalog_roles (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES catalog_projects(id),
    name TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, owner_id) REFERENCES catalog_memberships(project_id, user_id)
);
CREATE TABLE catalog_tables (
    project_id TEXT NOT NULL,
    warehouse_id TEXT NOT NULL,
    id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    creator_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'updating', 'deleting', 'deleted')),
    idempotency_key TEXT,
    request_hash BYTEA,
    delete_actor TEXT REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (warehouse_id, id),
    UNIQUE (project_id, warehouse_id, id),
    FOREIGN KEY (project_id, warehouse_id, namespace_id)
        REFERENCES catalog_namespaces(project_id, warehouse_id, id),
    FOREIGN KEY (project_id, owner_id) REFERENCES catalog_memberships(project_id, user_id),
    FOREIGN KEY (project_id, creator_id) REFERENCES catalog_memberships(project_id, user_id),
    CHECK ((idempotency_key IS NULL) = (request_hash IS NULL)),
    CHECK (idempotency_key IS NULL OR creator_id IS NOT NULL)
);
CREATE UNIQUE INDEX catalog_table_idempotency
    ON catalog_tables(project_id, warehouse_id, creator_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE TABLE catalog_pending_updates (
    project_id TEXT NOT NULL,
    warehouse_id TEXT NOT NULL,
    table_id TEXT NOT NULL,
    actor_id TEXT NOT NULL REFERENCES users(id),
    operation TEXT NOT NULL CHECK (operation IN ('owner', 'namespace')),
    owner_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    writes JSONB NOT NULL CHECK (jsonb_typeof(writes) = 'array'),
    deletes JSONB NOT NULL CHECK (jsonb_typeof(deletes) = 'array'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (warehouse_id, table_id),
    FOREIGN KEY (project_id, warehouse_id, table_id)
        REFERENCES catalog_tables(project_id, warehouse_id, id),
    FOREIGN KEY (project_id, owner_id) REFERENCES catalog_memberships(project_id, user_id),
    FOREIGN KEY (project_id, warehouse_id, namespace_id)
        REFERENCES catalog_namespaces(project_id, warehouse_id, id)
);
-- Random, actor-bound cursors do not expose identifiers of filtered objects.
CREATE TABLE catalog_list_cursors (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    warehouse_id TEXT NOT NULL,
    actor_id TEXT NOT NULL REFERENCES users(id),
    last_id TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    FOREIGN KEY (project_id, warehouse_id) REFERENCES catalog_warehouses(project_id, id)
);
CREATE INDEX catalog_list_cursors_expiry ON catalog_list_cursors(expires_at);
