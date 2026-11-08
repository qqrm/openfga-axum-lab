-- Add the Openlane task scenario without rewriting applied migration checksums
-- or deleting records from the retired demonstration. Old tables are not served.
CREATE TABLE organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users(id),
    authz_pending BOOLEAN NOT NULL DEFAULT false,
    authz_uncertain BOOLEAN NOT NULL DEFAULT false,
    authorization_version BIGINT NOT NULL DEFAULT 0 CHECK (authorization_version >= 0)
);
CREATE TABLE organization_members (
    organization_id TEXT NOT NULL REFERENCES organizations(id),
    user_id TEXT NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('member', 'admin')),
    PRIMARY KEY (organization_id, user_id)
);
CREATE TABLE organization_permissions (
    organization_id TEXT NOT NULL REFERENCES organizations(id),
    user_id TEXT NOT NULL REFERENCES users(id),
    relation TEXT NOT NULL CHECK (relation IN ('can_view_task', 'can_edit_task', 'can_delete_task')),
    PRIMARY KEY (organization_id, user_id, relation)
);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    task_status TEXT NOT NULL CHECK (task_status IN ('todo', 'in_progress', 'done')),
    assigner_id TEXT NOT NULL REFERENCES users(id),
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'deleting', 'deleted')),
    delete_actor TEXT REFERENCES users(id),
    idempotency_key TEXT,
    request_hash BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((idempotency_key IS NULL) = (request_hash IS NULL))
);
CREATE UNIQUE INDEX tasks_assigner_idempotency
    ON tasks (organization_id, assigner_id, idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX tasks_organization ON tasks (organization_id, id);
CREATE TABLE task_relations (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    user_id TEXT NOT NULL REFERENCES users(id),
    relation TEXT NOT NULL CHECK (relation IN ('viewer', 'editor', 'assignee', 'blocked')),
    PRIMARY KEY (task_id, user_id, relation)
);
-- Desired SQL state and its remote projection are staged in the same transaction.
-- Every public read/mutation fences a pending organization until acknowledgement.
CREATE TABLE authorization_jobs (
    organization_id TEXT PRIMARY KEY REFERENCES organizations(id),
    attempted BOOLEAN NOT NULL DEFAULT false,
    writes TEXT NOT NULL,
    deletes TEXT NOT NULL,
    delete_object TEXT,
    activate_task_id TEXT REFERENCES tasks(id),
    delete_task_id TEXT REFERENCES tasks(id),
    CHECK (activate_task_id IS NULL OR delete_task_id IS NULL)
);
