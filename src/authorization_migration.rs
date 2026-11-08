//! One-time, resumable SQL → OpenFGA migration.
//!
//! The source is an explicitly documented legacy SQL contract, populated from
//! existing relationships. It is frozen once a validated snapshot is bound to a
//! target. The application stays fenced until tuple reconciliation and all
//! old/new permission checks succeed. This is not a distributed transaction.
type ToolResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::Duration,
};
use tokio::time::timeout;

const MAX_RESPONSE_BYTES: usize = 1_048_576;
const MAX_SOURCE_ROWS: usize = 10_000;
const MAX_VERIFY_CHECKS: usize = 100_000;
const MAX_BATCH_SIZE: usize = 100;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct Tuple {
    user: String,
    relation: String,
    object: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Target {
    store_id: String,
    model_id: String,
    model_sha256: String,
}

struct Backend {
    client: Client,
    base: String,
    target: Target,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckDecision {
    allowed: bool,
    #[serde(default, rename = "resolution")]
    _resolution: Option<String>,
}

#[derive(Deserialize)]
struct StoreIdentity {
    id: String,
}

#[derive(Deserialize)]
struct ModelEnvelope {
    authorization_model: ModelRecord,
}

#[derive(Deserialize, Serialize)]
struct ModelRecord {
    id: String,
    schema_version: String,
    type_definitions: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conditions: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadPage {
    #[serde(default)]
    tuples: Vec<ReadTuple>,
    #[serde(default)]
    continuation_token: String,
}

#[derive(Deserialize)]
struct ReadTuple {
    key: WireTupleKey,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTupleKey {
    user: String,
    relation: String,
    object: String,
    #[serde(default)]
    condition: Option<Value>,
}
impl WireTupleKey {
    fn unconditional(self) -> ToolResult<Tuple> {
        if self.condition.is_some() {
            return Err("conditional tuple is outside the pinned migration contract".into());
        }
        Ok(Tuple {
            user: self.user,
            relation: self.relation,
            object: self.object,
        })
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

async fn success(response: Response) -> ToolResult<Response> {
    if response.status() != StatusCode::OK {
        return Err(format!("OpenFGA returned {}; expected 200", response.status()).into());
    }
    Ok(response)
}

fn decode_object<T: DeserializeOwned>(bytes: &[u8]) -> ToolResult<T> {
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err("OpenFGA response must be a JSON object".into());
    }
    Ok(serde_json::from_slice(bytes)?)
}

async fn json_body<T: DeserializeOwned>(mut response: Response) -> ToolResult<T> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.split(';').next().map(str::trim) != Some("application/json") {
        return Err("OpenFGA returned a non-JSON response".into());
    }
    timeout(Duration::from_secs(5), async move {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err::<T, Box<dyn std::error::Error + Send + Sync>>(
                    "OpenFGA response is too large".into(),
                );
            }
            bytes.extend_from_slice(&chunk);
        }
        decode_object(&bytes)
    })
    .await?
}

use crate::model_binding::policy_model;

impl Backend {
    fn endpoint(&self, suffix: &str) -> String {
        format!(
            "{}/stores/{}/{}",
            self.base.trim_end_matches('/'),
            self.target.store_id,
            suffix
        )
    }

    async fn validate_target(&self, expected_model: &Value) -> ToolResult {
        let store: StoreIdentity = json_body(
            success(
                self.client
                    .get(format!(
                        "{}/stores/{}",
                        self.base.trim_end_matches('/'),
                        self.target.store_id
                    ))
                    .send()
                    .await?,
            )
            .await?,
        )
        .await?;
        if store.id != self.target.store_id {
            return Err("OpenFGA store identity differs from the bound target".into());
        }
        let response: ModelEnvelope = json_body(
            success(
                self.client
                    .get(self.endpoint(&format!("authorization-models/{}", self.target.model_id)))
                    .send()
                    .await?,
            )
            .await?,
        )
        .await?;
        let actual = response.authorization_model;
        if actual.id != self.target.model_id
            || policy_model(&serde_json::to_value(actual)?)? != policy_model(expected_model)?
        {
            return Err("OpenFGA model differs from the pinned migration model".into());
        }
        Ok(())
    }

    async fn tuples(&self) -> ToolResult<BTreeSet<Tuple>> {
        let mut result = BTreeSet::new();
        let mut cursor = String::new();
        let mut seen_cursors = BTreeSet::new();
        for _ in 0..=MAX_SOURCE_ROWS {
            let body = json!({"page_size": 100, "continuation_token": cursor, "consistency": "HIGHER_CONSISTENCY"});
            let response = success(
                self.client
                    .post(self.endpoint("read"))
                    .json(&body)
                    .send()
                    .await?,
            )
            .await?;
            let page: ReadPage = json_body(response).await?;
            if page.tuples.is_empty() && !page.continuation_token.is_empty() {
                return Err("OpenFGA returned an incomplete empty tuple page".into());
            }
            for tuple in page.tuples {
                if !result.insert(tuple.key.unconditional()?) {
                    return Err(
                        "OpenFGA returned a duplicate tuple while reading the target".into(),
                    );
                }
                if result.len() > MAX_SOURCE_ROWS {
                    return Err("target tuple limit exceeded".into());
                }
            }
            if page.continuation_token.is_empty() {
                return Ok(result);
            }
            if !seen_cursors.insert(page.continuation_token.clone()) {
                return Err("OpenFGA repeated a continuation token".into());
            }
            cursor = page.continuation_token;
        }
        Err("OpenFGA pagination did not terminate".into())
    }

    async fn write(&self, tuples: &[Tuple]) -> ToolResult {
        if tuples.is_empty() || tuples.len() > MAX_BATCH_SIZE {
            return Err("invalid migration batch size".into());
        }
        let body = json!({"authorization_model_id": self.target.model_id, "writes": {"tuple_keys": tuples, "on_duplicate": "ignore"}});
        let response = success(
            self.client
                .post(self.endpoint("write"))
                .json(&body)
                .send()
                .await?,
        )
        .await?;
        // A truncated/non-JSON response is an uncertain outcome. The durable
        // checkpoint remains unchanged and a subsequent process repeats the
        // same idempotent batch; no access is activated on this path.
        let body: Value = json_body(response).await?;
        if body != json!({}) {
            return Err("invalid OpenFGA Write response".into());
        }
        Ok(())
    }

    async fn check(&self, user: &str, relation: &str, object: &str) -> ToolResult<bool> {
        let response = success(self.client.post(self.endpoint("check")).json(&json!({
            "authorization_model_id": self.target.model_id,
            "consistency": "HIGHER_CONSISTENCY",
            "tuple_key": {"user": format!("user:{user}"), "relation": relation, "object": object},
        })).send().await?).await?;
        let result: CheckDecision = json_body(response).await?;
        Ok(result.allowed)
    }
}

/// Synthetic existing catalog/ACL data using the pinned upstream model's
/// resource and relation names. This is intentionally separate from tuple
/// generation: the migration reads these SQL rows, never a tuple JSON fixture.
pub async fn seed(db: &PgPool) -> ToolResult {
    let mut transaction = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(6041927238613300)")
        .execute(&mut *transaction)
        .await?;
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_projects")
        .fetch_one(&mut *transaction)
        .await?;
    if existing != 0 {
        return Ok(());
    }
    sqlx::raw_sql("\
INSERT INTO catalog_projects(id,name) VALUES ('acme','Acme Analytics'),('other','Other tenant');
INSERT INTO catalog_memberships(project_id,user_id) VALUES ('acme','alice'),('acme','bob'),('acme','carol'),('acme','dave'),('other','eve');
INSERT INTO catalog_warehouses(id,project_id,name,owner_id) VALUES ('analytics','acme','Analytics warehouse','alice'),('external','other','Other warehouse','eve');
INSERT INTO catalog_namespaces(id,project_id,warehouse_id,name,owner_id) VALUES ('metrics','acme','analytics','Metrics','alice'),('restricted','acme','analytics','Restricted','alice'),('other-metrics','other','external','Metrics','eve');
INSERT INTO catalog_roles(id,project_id,name,owner_id) VALUES ('analysts','acme','Analysts','alice'),('analyst-members','acme','Analyst members','alice'),('other-analysts','other','Analysts','eve');
INSERT INTO catalog_tables(project_id,warehouse_id,id,namespace_id,name,description,owner_id,status) VALUES
 ('acme','analytics','requests','metrics','Requests','Synthetic HTTP request observations','carol','active'),
 ('acme','analytics','latency','metrics','Latency','Synthetic latency measurements','alice','active'),
 ('acme','analytics','errors','metrics','Errors','Synthetic error observations','bob','active'),
 ('other','external','requests','other-metrics','Other requests','Synthetic observations from a separate tenant','eve','active');
INSERT INTO legacy_project_grants(project_id,subject_kind,subject_id,relation) VALUES ('acme','user','alice','project_admin'),('acme','role','analyst-members','select'),('acme','role','analysts','create'),('other','user','eve','project_admin');
INSERT INTO legacy_role_assignments(role_id,subject_kind,subject_id) VALUES ('analysts','user','bob'),('analyst-members','role','analysts');
INSERT INTO legacy_resource_grants(object_kind,object_id,subject_kind,subject_id,relation) VALUES
 ('lakekeeper_table','analytics/latency','user','dave','select');")
        .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(())
}

/// A runtime gate independent of the mutable decision-cache revision.
/// Readiness and every authorization check must use the same target binding.
pub async fn ensure_active(db: &PgPool, store: &str, model: &str, model_hash: &str) -> ToolResult {
    let active: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM authorization_migration WHERE singleton AND status='active' AND store_id=$1 AND model_id=$2 AND model_sha256=$3)")
        .bind(store).bind(model).bind(model_hash).fetch_one(db).await?;
    if !active {
        return Err("authorization migration is not active for the configured target".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
struct Object {
    kind: String,
    id: String,
    project: String,
    owner: Option<String>,
    parent: Option<String>,
}

impl Object {
    fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

#[derive(Clone, Debug, Serialize)]
struct Grant {
    object: String,
    subject_kind: String,
    subject_id: String,
    relation: String,
}

impl Grant {
    fn subject(&self) -> String {
        if self.subject_kind == "role" {
            format!("role:{}#assignee", self.subject_id)
        } else {
            format!("user:{}", self.subject_id)
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct Source {
    users: Vec<String>,
    memberships: BTreeSet<(String, String)>,
    objects: BTreeMap<String, Object>,
    grants: Vec<Grant>,
}

impl Source {
    async fn read(connection: &mut sqlx::PgConnection) -> ToolResult<Self> {
        let users: Vec<String> = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 10001")
            .fetch_all(&mut *connection)
            .await?;
        let memberships = sqlx::query_as::<_,(String,String)>("SELECT project_id,user_id FROM catalog_memberships ORDER BY project_id,user_id LIMIT 10001").fetch_all(&mut *connection).await?.into_iter().collect();
        let mut objects = BTreeMap::new();
        for id in sqlx::query_scalar::<_, String>(
            "SELECT id FROM catalog_projects ORDER BY id LIMIT 10001",
        )
        .fetch_all(&mut *connection)
        .await?
        {
            let object = Object {
                kind: "project".into(),
                project: id.clone(),
                id,
                owner: None,
                parent: None,
            };
            objects.insert(object.key(), object);
        }
        for row in sqlx::query("SELECT 'warehouse' AS kind,id,project_id,owner_id FROM catalog_warehouses UNION ALL SELECT 'role',id,project_id,owner_id FROM catalog_roles ORDER BY kind,id LIMIT 10001").fetch_all(&mut *connection).await? {
            let project:String = row.try_get("project_id")?;
            let object = Object { kind:row.try_get("kind")?, id:row.try_get("id")?, owner:Some(row.try_get("owner_id")?), parent:Some(format!("project:{project}")), project };
            objects.insert(object.key(),object);
        }
        for (id,project,warehouse,owner) in sqlx::query_as::<_,(String,String,String,String)>("SELECT id,project_id,warehouse_id,owner_id FROM catalog_namespaces ORDER BY id LIMIT 10001").fetch_all(&mut *connection).await? {
            let object = Object { kind:"namespace".into(), id, project, owner:Some(owner), parent:Some(format!("warehouse:{warehouse}")) };
            objects.insert(object.key(),object);
        }
        for (id,project,warehouse,namespace,owner,status) in sqlx::query_as::<_,(String,String,String,String,String,String)>("SELECT id,project_id,warehouse_id,namespace_id,owner_id,status FROM catalog_tables ORDER BY warehouse_id,id LIMIT 10001").fetch_all(&mut *connection).await? {
            if status != "active" { return Err("legacy catalog contains an unresolved or deleted table; resolve lifecycle state before cutover".into()); }
            let object = Object { kind:"lakekeeper_table".into(), id:format!("{warehouse}/{id}"), project, owner:Some(owner), parent:Some(format!("namespace:{namespace}")) };
            objects.insert(object.key(),object);
        }
        let mut grants = Vec::new();
        for (project,subject_kind,subject_id,relation) in sqlx::query_as::<_,(String,String,String,String)>("SELECT project_id,subject_kind,subject_id,relation FROM legacy_project_grants ORDER BY project_id,subject_kind,subject_id,relation LIMIT 10001").fetch_all(&mut *connection).await? {
            grants.push(Grant { object:format!("project:{project}"),subject_kind,subject_id,relation });
        }
        for (kind,id,subject_kind,subject_id,relation) in sqlx::query_as::<_,(String,String,String,String,String)>("SELECT object_kind,object_id,subject_kind,subject_id,relation FROM legacy_resource_grants ORDER BY object_kind,object_id,subject_kind,subject_id,relation LIMIT 10001").fetch_all(&mut *connection).await? {
            if !matches!(kind.as_str(),"warehouse"|"namespace"|"lakekeeper_table") { return Err("unsupported legacy resource kind".into()); }
            grants.push(Grant { object:format!("{kind}:{id}"),subject_kind,subject_id,relation });
        }
        for (role,subject_kind,subject_id) in sqlx::query_as::<_,(String,String,String)>("SELECT role_id,subject_kind,subject_id FROM legacy_role_assignments ORDER BY role_id,subject_kind,subject_id LIMIT 10001").fetch_all(&mut *connection).await? {
            grants.push(Grant { object:format!("role:{role}"),subject_kind,subject_id,relation:"assignee".into() });
        }
        Ok(Self {
            users,
            memberships,
            objects,
            grants,
        })
    }
}

impl Source {
    fn validate(&self) -> ToolResult {
        let rows =
            self.users.len() + self.memberships.len() + self.objects.len() + self.grants.len();
        if rows > MAX_SOURCE_ROWS || self.objects.is_empty() || self.users.is_empty() {
            return Err(
                "legacy authorization source is empty or exceeds the bounded migration size".into(),
            );
        }
        if self.users.iter().any(|user| !identifier(user)) {
            return Err("invalid legacy user identifier".into());
        }
        for (project, user) in &self.memberships {
            if !self.objects.contains_key(&format!("project:{project}"))
                || !self.users.contains(user)
            {
                return Err("membership has a missing project or subject".into());
            }
        }
        for object in self.objects.values() {
            let valid_id = if object.kind == "lakekeeper_table" {
                let parts: Vec<_> = object.id.split('/').collect();
                parts.len() == 2 && parts.iter().all(|part| identifier(part))
            } else {
                identifier(&object.id)
            };
            if !valid_id || !identifier(&object.project) {
                return Err("invalid legacy object identifier".into());
            }
            if let Some(owner) = &object.owner {
                self.validate_subject("user", owner, &object.project)?;
            }
            if let Some(parent) = &object.parent {
                let parent = self
                    .objects
                    .get(parent)
                    .ok_or("legacy resource parent is missing")?;
                if parent.project != object.project {
                    return Err("cross-tenant parent link rejected".into());
                }
                if object.kind == "lakekeeper_table"
                    && parent.parent.as_deref()
                        != object
                            .id
                            .split_once('/')
                            .map(|(warehouse, _)| format!("warehouse:{warehouse}"))
                            .as_deref()
                {
                    return Err("table namespace belongs to a different warehouse".into());
                }
            }
        }
        for grant in &self.grants {
            let object = self
                .objects
                .get(&grant.object)
                .ok_or("legacy grant targets a missing object")?;
            self.validate_subject(&grant.subject_kind, &grant.subject_id, &object.project)?;
            let allowed: &[&str] = match object.kind.as_str() {
                "project" => &[
                    "project_admin",
                    "data_admin",
                    "security_admin",
                    "create",
                    "describe",
                    "select",
                    "write",
                    "modify",
                ],
                "warehouse" | "namespace" => &[
                    "create",
                    "describe",
                    "select",
                    "write",
                    "modify",
                    "manage_grants",
                    "pass_grants",
                ],
                "lakekeeper_table" => &[
                    "describe",
                    "select",
                    "write",
                    "modify",
                    "manage_grants",
                    "pass_grants",
                ],
                "role" => &["assignee"],
                _ => return Err("unsupported legacy object kind".into()),
            };
            if !allowed.contains(&grant.relation.as_str()) {
                return Err("unsupported or escalating legacy relation rejected".into());
            }
        }
        for object in self.objects.values().filter(|object| object.kind == "role") {
            self.validate_role_tree(&object.id, &mut BTreeSet::new())?;
        }
        Ok(())
    }

    fn validate_subject(&self, kind: &str, id: &str, project: &str) -> ToolResult {
        if !identifier(id) {
            return Err("wildcard or invalid legacy subject rejected".into());
        }
        match kind {
            "user"
                if self.users.iter().any(|user| user == id)
                    && self.memberships.contains(&(project.into(), id.into())) =>
            {
                Ok(())
            }
            "role"
                if self
                    .objects
                    .get(&format!("role:{id}"))
                    .is_some_and(|role| role.project == project) =>
            {
                Ok(())
            }
            _ => Err("missing or cross-tenant legacy subject rejected".into()),
        }
    }

    fn validate_role_tree(&self, role: &str, visiting: &mut BTreeSet<String>) -> ToolResult {
        if !visiting.insert(role.into()) {
            return Err("cyclic role inheritance rejected".into());
        }
        if visiting.len() > 32 {
            return Err("role inheritance depth exceeds supported bound".into());
        }
        for grant in self
            .grants
            .iter()
            .filter(|grant| grant.object == format!("role:{role}") && grant.subject_kind == "role")
        {
            self.validate_role_tree(&grant.subject_id, visiting)?;
        }
        visiting.remove(role);
        Ok(())
    }

    fn tuples(&self) -> ToolResult<BTreeSet<Tuple>> {
        let mut result = BTreeSet::new();
        for object in self.objects.values() {
            let key = object.key();
            if let Some(owner) = &object.owner {
                result.insert(Tuple {
                    user: format!("user:{owner}"),
                    relation: "ownership".into(),
                    object: key.clone(),
                });
            }
            if let Some(parent) = &object.parent {
                let relation = if matches!(object.kind.as_str(), "warehouse" | "role") {
                    "project"
                } else {
                    "parent"
                };
                result.insert(Tuple {
                    user: parent.clone(),
                    relation: relation.into(),
                    object: key.clone(),
                });
                let reverse = match object.kind.as_str() {
                    "warehouse" => Some("warehouse"),
                    "namespace" => Some("namespace"),
                    "lakekeeper_table" => Some("child"),
                    _ => None,
                };
                if let Some(reverse) = reverse {
                    result.insert(Tuple {
                        user: key,
                        relation: reverse.into(),
                        object: parent.clone(),
                    });
                }
            }
        }
        for grant in &self.grants {
            result.insert(Tuple {
                user: grant.subject(),
                relation: grant.relation.clone(),
                object: grant.object.clone(),
            });
        }
        if result.len() > MAX_SOURCE_ROWS {
            return Err("tuple plan exceeds supported migration size".into());
        }
        Ok(result)
    }

    fn role_member(&self, user: &str, role: &str) -> bool {
        let Some(object) = self.objects.get(&format!("role:{role}")) else {
            return false;
        };
        object.owner.as_deref() == Some(user)
            || self.grants.iter().any(|grant| {
                grant.object == object.key()
                    && match grant.subject_kind.as_str() {
                        "user" => grant.subject_id == user,
                        "role" => self.role_member(user, &grant.subject_id),
                        _ => false,
                    }
            })
    }

    fn direct(&self, user: &str, object: &Object, relation: &str) -> bool {
        self.grants.iter().any(|grant| {
            grant.object == object.key()
                && grant.relation == relation
                && match grant.subject_kind.as_str() {
                    "user" => grant.subject_id == user,
                    "role" => self.role_member(user, &grant.subject_id),
                    _ => false,
                }
        })
    }

    /// Independent legacy ACL evaluator. It reads normalized SQL ownership,
    /// memberships, role assignments and parent references, not generated
    /// tuples or the OpenFGA model expression tree. Unsupported source policy
    /// features are rejected rather than interpreted as an empty grant.
    fn capabilities(&self, user: &str, object: &Object) -> Capabilities {
        if !self
            .memberships
            .contains(&(object.project.clone(), user.into()))
        {
            return Capabilities::default();
        }
        let own = object.owner.as_deref() == Some(user);
        let parent = object
            .parent
            .as_ref()
            .and_then(|parent| self.objects.get(parent))
            .map(|parent| self.capabilities(user, parent))
            .unwrap_or_default();
        let direct = |relation| self.direct(user, object, relation);
        let admin = object.kind == "project" && direct("project_admin");
        let data_admin = object.kind == "project" && (admin || direct("data_admin"));
        let security = object.kind == "project" && (admin || direct("security_admin"));
        let modify = own
            || direct("modify")
            || admin
            || parent.modify
            || (object.kind == "warehouse" && parent.data_admin);
        let write = own || direct("write") || modify || parent.write;
        let select = own || direct("select") || write || parent.select;
        let describe =
            own || direct("describe") || select || parent.describe || data_admin || security;
        let create = own || direct("create") || parent.create || data_admin;
        let manage = own
            || direct("manage_grants")
            || if object.kind == "warehouse" {
                parent.security
            } else {
                parent.manage
            };
        Capabilities {
            modify,
            write,
            select,
            describe,
            create,
            manage,
            pass: direct("pass_grants"),
            security,
            data_admin,
        }
    }

    fn visible_below(&self, user: &str, object: &Object) -> bool {
        object.owner.as_deref() == Some(user)
            || ["describe", "select", "write", "modify", "create"]
                .iter()
                .any(|relation| self.direct(user, object, relation))
            || self
                .objects
                .values()
                .filter(|child| {
                    child.parent.as_deref() == Some(&object.key()) && child.kind != "role"
                })
                .any(|child| self.visible_below(user, child))
    }

    fn legacy_check(&self, user: &str, object: &Object, action: &str) -> ToolResult<bool> {
        let permission = self.capabilities(user, object);
        if !self
            .memberships
            .contains(&(object.project.clone(), user.into()))
        {
            return Ok(false);
        }
        let child_visible = self
            .objects
            .values()
            .filter(|child| child.parent.as_deref() == Some(&object.key()) && child.kind != "role")
            .any(|child| self.visible_below(user, child));
        let metadata = permission.describe
            || (object.kind != "lakekeeper_table" && (permission.create || child_visible));
        let manage = permission.manage;
        if object.kind == "role" {
            let project = self
                .objects
                .get(&format!("project:{}", object.project))
                .ok_or("missing role project")?;
            return Ok(match action {
                "can_assume" => self.role_member(user, &object.id),
                "can_grant_assignee" | "can_change_ownership" | "can_delete" | "can_update" => {
                    object.owner.as_deref() == Some(user)
                        || self.capabilities(user, project).security
                }
                "can_read" | "can_read_assignments" => {
                    self.legacy_check(user, project, "can_get_metadata")?
                }
                _ => return Err("unsupported legacy role check".into()),
            });
        }
        Ok(match action {
            "can_get_metadata"
            | "can_include_in_list"
            | "can_get_config"
            | "can_list_warehouses"
            | "can_list_namespaces"
            | "can_list_tables"
            | "can_use" => metadata,
            "can_read_data" => permission.select,
            "can_write_data" | "can_commit" => permission.write,
            "can_drop" | "can_rename" => permission.modify,
            "can_create_table"
            | "can_create_namespace"
            | "can_create_warehouse"
            | "can_accept_moved_tabular" => permission.create,
            "can_change_ownership" | "can_grant_pass_grants" | "can_grant_manage_grants" => manage,
            "can_read_assignments" if object.kind == "project" => permission.security,
            "can_read_assignments" => {
                manage
                    || permission.pass
                        && (permission.describe
                            || permission.select
                            || permission.write
                            || permission.modify
                            || object.kind != "lakekeeper_table" && permission.create)
            }
            "can_move" => manage && permission.modify,
            "can_revoke_select"
            | "can_revoke_modify"
            | "can_revoke_write"
            | "can_revoke_describe" => manage,
            "can_grant_select" => {
                manage
                    || permission.pass && permission.select
                    || object.kind == "project" && permission.security
            }
            "can_grant_modify" => {
                manage
                    || permission.pass && permission.modify
                    || object.kind == "project" && permission.security
            }
            "can_grant_write" => {
                manage
                    || permission.pass && permission.write
                    || object.kind == "project" && permission.security
            }
            "can_grant_describe" => {
                manage
                    || permission.pass && permission.describe
                    || object.kind == "project" && permission.security
            }
            _ => return Err("unsupported legacy permission check".into()),
        })
    }
}

#[derive(Default)]
struct Capabilities {
    modify: bool,
    write: bool,
    select: bool,
    describe: bool,
    create: bool,
    manage: bool,
    pass: bool,
    security: bool,
    data_admin: bool,
}

fn actions(kind: &str) -> &'static [&'static str] {
    match kind {
        "project" => &[
            "can_get_metadata",
            "can_include_in_list",
            "can_list_warehouses",
            "can_read_assignments",
            "can_create_warehouse",
            "can_grant_select",
            "can_grant_modify",
            "can_grant_write",
            "can_grant_describe",
        ],
        "warehouse" => &[
            "can_get_metadata",
            "can_include_in_list",
            "can_get_config",
            "can_list_namespaces",
            "can_create_namespace",
            "can_change_ownership",
            "can_read_assignments",
            "can_grant_pass_grants",
            "can_grant_manage_grants",
            "can_grant_select",
            "can_grant_modify",
            "can_grant_write",
            "can_grant_describe",
            "can_revoke_select",
            "can_revoke_modify",
            "can_revoke_write",
            "can_revoke_describe",
        ],
        "namespace" => &[
            "can_get_metadata",
            "can_include_in_list",
            "can_list_tables",
            "can_create_table",
            "can_accept_moved_tabular",
            "can_change_ownership",
            "can_move",
            "can_read_assignments",
            "can_grant_pass_grants",
            "can_grant_manage_grants",
            "can_grant_select",
            "can_grant_modify",
            "can_grant_write",
            "can_grant_describe",
            "can_revoke_select",
            "can_revoke_modify",
            "can_revoke_write",
            "can_revoke_describe",
        ],
        "lakekeeper_table" => &[
            "can_get_metadata",
            "can_include_in_list",
            "can_read_data",
            "can_write_data",
            "can_commit",
            "can_drop",
            "can_rename",
            "can_change_ownership",
            "can_move",
            "can_read_assignments",
            "can_grant_pass_grants",
            "can_grant_manage_grants",
            "can_grant_select",
            "can_grant_modify",
            "can_grant_write",
            "can_grant_describe",
            "can_revoke_select",
            "can_revoke_modify",
            "can_revoke_write",
            "can_revoke_describe",
        ],
        "role" => &[
            "can_assume",
            "can_grant_assignee",
            "can_change_ownership",
            "can_delete",
            "can_update",
            "can_read",
            "can_read_assignments",
        ],
        _ => &[],
    }
}

#[derive(Debug, Serialize)]
pub struct MigrationReport {
    pub already_active: bool,
    pub tuple_count: usize,
    pub verified_checks: usize,
    pub source_sha256: String,
}

const SOURCE_TABLES: &str = "LOCK TABLE users, catalog_projects, catalog_memberships, catalog_warehouses, catalog_namespaces, catalog_tables, catalog_roles, legacy_project_grants, legacy_resource_grants, legacy_role_assignments IN SHARE MODE";

/// Migrate a stable, validated SQL snapshot into a dedicated OpenFGA store.
/// The model provisioner creates an empty target; this function alone activates
/// the runtime gate. HTTP is used for this operational tool with either runtime
/// transport. The two transports share the same store and authorization model.
pub async fn migrate(
    db: &PgPool,
    base: &str,
    config_path: &Path,
    model_path: &Path,
) -> ToolResult<MigrationReport> {
    migrate_with_batch_size(db, base, config_path, model_path, MAX_BATCH_SIZE).await
}

async fn migrate_with_batch_size(
    db: &PgPool,
    base: &str,
    config_path: &Path,
    model_path: &Path,
    batch_size: usize,
) -> ToolResult<MigrationReport> {
    if !(1..=MAX_BATCH_SIZE).contains(&batch_size) {
        return Err("AUTHZ_MIGRATION_BATCH_SIZE must be between 1 and 100".into());
    }
    let target: Target = serde_json::from_slice(&fs::read(config_path)?)?;
    if !identifier(&target.store_id) || !identifier(&target.model_id) {
        return Err("invalid target identifiers".into());
    }
    let model_bytes = fs::read(model_path)?;
    // The legacy evaluator supports this exact public model version. Changing
    // its graph requires an explicit reviewed policy/migration update.
    const PINNED_MODEL_SHA256: &str =
        "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d";
    if target.model_sha256 != digest(&model_bytes) || target.model_sha256 != PINNED_MODEL_SHA256 {
        return Err("model hash differs from the pinned Lakekeeper migration contract".into());
    }
    let expected_model: Value = serde_json::from_slice(&model_bytes)?;
    let backend = Backend {
        client: Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()?,
        base: base.into(),
        target,
    };
    // Transaction-scoped lock releases even if this process is cancelled. It
    // is deliberately independent from the durable checkpoints below.
    let mut run_lock = db.begin().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(6041927238613301)")
        .fetch_one(&mut *run_lock)
        .await?;
    if !acquired {
        return Err("another authorization migration is already running".into());
    }
    backend.validate_target(&expected_model).await?;
    let mut snapshot = db.begin().await?;
    sqlx::query(SOURCE_TABLES).execute(&mut *snapshot).await?;
    let row=sqlx::query("SELECT status,store_id,model_id,model_sha256,source_sha256,next_tuple,tuple_count,verified_checks FROM authorization_migration WHERE singleton FOR UPDATE").fetch_one(&mut *snapshot).await?;
    let status: String = row.try_get("status")?;
    let bound_store: Option<String> = row.try_get("store_id")?;
    let bound_model: Option<String> = row.try_get("model_id")?;
    let bound_model_hash: Option<String> = row.try_get("model_sha256")?;
    if status != "pending"
        && (bound_store.as_deref() != Some(&backend.target.store_id)
            || bound_model.as_deref() != Some(&backend.target.model_id)
            || bound_model_hash.as_deref() != Some(&backend.target.model_sha256))
    {
        return Err("migration target differs from the persisted store/model binding".into());
    }
    if status == "active" {
        // Runtime grants/owners can legitimately change after cutover. A
        // rerun must never resurrect an archived grant or overwrite a revoke.
        return Ok(MigrationReport {
            already_active: true,
            tuple_count: usize::try_from(row.try_get::<i64, _>("tuple_count")?)?,
            verified_checks: usize::try_from(row.try_get::<i64, _>("verified_checks")?)?,
            source_sha256: row.try_get("source_sha256")?,
        });
    }
    let source = Source::read(&mut snapshot).await?;
    source.validate()?;
    let source_hash = digest(&serde_json::to_vec(&source)?);
    let expected_tuples = source.tuples()?;
    let tuples: Vec<_> = expected_tuples.iter().cloned().collect();
    let mut users = source.users.clone();
    let outsider = format!("migration-outsider-{}", uuid::Uuid::new_v4().simple());
    users.push(outsider);
    let checks = users
        .len()
        .checked_mul(
            source
                .objects
                .values()
                .map(|object| actions(&object.kind).len())
                .sum(),
        )
        .ok_or("verification matrix overflow")?;
    if checks > MAX_VERIFY_CHECKS {
        return Err("legacy permission matrix exceeds supported migration bound".into());
    }
    let mut next = usize::try_from(row.try_get::<i64, _>("next_tuple")?)?;
    if status == "pending" {
        if !backend.tuples().await?.is_empty() {
            return Err("initial migration requires an empty dedicated OpenFGA store".into());
        }
        sqlx::query("UPDATE authorization_migration SET status='writing',store_id=$1,model_id=$2,model_sha256=$3,source_sha256=$4,tuple_count=$5,next_tuple=0,verified_checks=0,updated_at=now() WHERE singleton")
            .bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).bind(&source_hash).bind(i64::try_from(tuples.len())?).execute(&mut *snapshot).await?;
        next = 0;
    } else if row
        .try_get::<Option<String>, _>("source_sha256")?
        .as_deref()
        != Some(&source_hash)
        || usize::try_from(row.try_get::<i64, _>("tuple_count")?)? != tuples.len()
        || next > tuples.len()
    {
        return Err("legacy source or tuple plan differs from the persisted checkpoint".into());
    }
    snapshot.commit().await?;
    // From this point any error, timeout, cancellation or process death leaves
    // writing/verifying durable. The application keeps rejecting all access.
    let present = backend.tuples().await?;
    if !present.is_subset(&expected_tuples) {
        return Err("unexpected target tuples rejected before migration resume".into());
    }
    // A committed checkpoint requires every earlier tuple to remain present.
    // Missing acknowledged tuples are drift, not a reason to invent a repair.
    if tuples[..next].iter().any(|tuple| !present.contains(tuple)) {
        return Err("target drift: a checkpointed tuple is missing".into());
    }
    while next < tuples.len() {
        let end = (next + batch_size).min(tuples.len());
        backend.write(&tuples[next..end]).await?;
        let advanced = sqlx::query("UPDATE authorization_migration SET next_tuple=$1,status='writing',updated_at=now() WHERE singleton AND source_sha256=$2 AND store_id=$3 AND model_id=$4 AND model_sha256=$5 AND next_tuple=$6 AND status IN ('writing','verifying')")
            .bind(i64::try_from(end)?).bind(&source_hash).bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).bind(i64::try_from(next)?).execute(db).await?.rows_affected();
        if advanced != 1 {
            return Err(
                "migration checkpoint binding changed after remote write; access remains inactive"
                    .into(),
            );
        }
        next = end;
    }
    let verifying = sqlx::query("UPDATE authorization_migration SET status='verifying',verified_checks=0,updated_at=now() WHERE singleton AND status IN ('writing','verifying') AND source_sha256=$1 AND store_id=$2 AND model_id=$3 AND model_sha256=$4 AND next_tuple=tuple_count")
        .bind(&source_hash).bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).execute(db).await?.rows_affected();
    if verifying != 1 {
        return Err("migration verification preconditions changed".into());
    }
    if backend.tuples().await? != expected_tuples {
        return Err("target tuple set differs from the validated SQL source".into());
    }
    let mut verified = 0usize;
    for user in &users {
        for object in source.objects.values() {
            for action in actions(&object.kind) {
                let expected = source.legacy_check(user, object, action)?;
                let actual = backend.check(user, action, &object.key()).await?;
                if actual != expected {
                    return Err(format!(
                        "permission equivalence failed at check {}; target stays inactive",
                        verified + 1
                    )
                    .into());
                }
                verified += 1;
            }
        }
    }
    // Confirm target integrity again after verification. Administrative writes
    // to this dedicated store must be quiesced during cutover. OpenFGA and
    // PostgreSQL do not share a transaction or a write lock.
    if backend.tuples().await? != expected_tuples {
        return Err("target graph changed during permission verification".into());
    }
    let mut activation = db.begin().await?;
    sqlx::query(SOURCE_TABLES).execute(&mut *activation).await?;
    let final_source = Source::read(&mut activation).await?;
    if digest(&serde_json::to_vec(&final_source)?) != source_hash {
        return Err("legacy source changed during permission verification".into());
    }
    let changed=sqlx::query("UPDATE authorization_migration SET status='active',verified_checks=$1,activated_at=now(),updated_at=now() WHERE singleton AND status='verifying' AND source_sha256=$2 AND store_id=$3 AND model_id=$4 AND model_sha256=$5 AND next_tuple=tuple_count")
        .bind(i64::try_from(verified)?).bind(&source_hash).bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).execute(&mut *activation).await?.rows_affected();
    if changed != 1 {
        return Err("migration activation preconditions changed".into());
    }
    activation.commit().await?;
    tracing::info!(
        tuple_count = tuples.len(),
        verified_checks = verified,
        "authorization migration activated after permission equivalence"
    );
    Ok(MigrationReport {
        already_active: false,
        tuple_count: tuples.len(),
        verified_checks: verified,
        source_sha256: source_hash,
    })
}

/// CLI entry point. Existing SQL data is mandatory; this command never creates
/// synthetic users or silently replaces a target/source binding.
pub async fn run() -> ToolResult {
    let database = std::env::var("DATABASE_URL")?;
    let db = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database)
        .await?;
    let migrations = std::env::var("MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".into());
    sqlx::migrate::Migrator::new(Path::new(&migrations))
        .await?
        .run(&db)
        .await?;
    let base = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
    let config = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let model = std::env::var("FGA_MODEL").unwrap_or_else(|_| "/app/fga/model.json".into());
    let batch = std::env::var("AUTHZ_MIGRATION_BATCH_SIZE")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(MAX_BATCH_SIZE);
    let report =
        migrate_with_batch_size(&db, &base, Path::new(&config), Path::new(&model), batch).await?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        let mut objects = BTreeMap::new();
        for (kind, id, project, owner, parent) in [
            ("project", "acme", "acme", None, None),
            ("project", "other", "other", None, None),
            (
                "warehouse",
                "analytics",
                "acme",
                Some("alice"),
                Some("project:acme"),
            ),
            (
                "warehouse",
                "external",
                "other",
                Some("eve"),
                Some("project:other"),
            ),
            (
                "namespace",
                "metrics",
                "acme",
                Some("alice"),
                Some("warehouse:analytics"),
            ),
            (
                "namespace",
                "foreign",
                "other",
                Some("eve"),
                Some("warehouse:external"),
            ),
            (
                "lakekeeper_table",
                "analytics/requests",
                "acme",
                Some("carol"),
                Some("namespace:metrics"),
            ),
            (
                "lakekeeper_table",
                "external/requests",
                "other",
                Some("eve"),
                Some("namespace:foreign"),
            ),
            (
                "role",
                "analysts",
                "acme",
                Some("alice"),
                Some("project:acme"),
            ),
            (
                "role",
                "nested",
                "acme",
                Some("alice"),
                Some("project:acme"),
            ),
            (
                "role",
                "other-role",
                "other",
                Some("eve"),
                Some("project:other"),
            ),
        ] {
            let object = Object {
                kind: kind.into(),
                id: id.into(),
                project: project.into(),
                owner: owner.map(str::to_owned),
                parent: parent.map(str::to_owned),
            };
            objects.insert(object.key(), object);
        }
        Source {
            users: vec!["alice", "bob", "carol", "eve"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            memberships: BTreeSet::from([
                ("acme".into(), "alice".into()),
                ("acme".into(), "bob".into()),
                ("acme".into(), "carol".into()),
                ("other".into(), "eve".into()),
            ]),
            objects,
            grants: vec![
                Grant {
                    object: "project:acme".into(),
                    subject_kind: "user".into(),
                    subject_id: "alice".into(),
                    relation: "project_admin".into(),
                },
                Grant {
                    object: "role:analysts".into(),
                    subject_kind: "user".into(),
                    subject_id: "bob".into(),
                    relation: "assignee".into(),
                },
                Grant {
                    object: "role:nested".into(),
                    subject_kind: "role".into(),
                    subject_id: "analysts".into(),
                    relation: "assignee".into(),
                },
                Grant {
                    object: "warehouse:analytics".into(),
                    subject_kind: "role".into(),
                    subject_id: "nested".into(),
                    relation: "select".into(),
                },
            ],
        }
    }

    #[test]
    fn legacy_roles_owners_and_parents_preserve_separate_privilege_tiers() {
        let source = source();
        source.validate().unwrap();
        let table = &source.objects["lakekeeper_table:analytics/requests"];
        assert!(source.legacy_check("bob", table, "can_read_data").unwrap());
        assert!(!source.legacy_check("bob", table, "can_write_data").unwrap());
        assert!(
            !source
                .legacy_check("bob", table, "can_grant_select")
                .unwrap()
        );
        assert!(source.legacy_check("carol", table, "can_drop").unwrap());
        assert!(
            source
                .legacy_check("alice", table, "can_change_ownership")
                .unwrap()
        );
        for action in actions("lakekeeper_table") {
            assert!(!source.legacy_check("eve", table, action).unwrap());
            assert!(!source.legacy_check("unknown", table, action).unwrap());
        }
        let tuples = source.tuples().unwrap();
        assert!(tuples.contains(&Tuple {
            user: "role:analysts#assignee".into(),
            relation: "assignee".into(),
            object: "role:nested".into()
        }));
        assert!(tuples.contains(&Tuple {
            user: "lakekeeper_table:analytics/requests".into(),
            relation: "child".into(),
            object: "namespace:metrics".into()
        }));
    }

    #[test]
    fn legacy_source_rejects_cross_tenant_unknown_wildcard_and_escalating_grants() {
        for (kind, id, relation) in [
            ("role", "other-role", "select"),
            ("user", "eve", "select"),
            ("user", "missing", "select"),
            ("user", "*", "select"),
            ("user", "bob", "ownership"),
            ("user", "bob", "can_grant_select"),
        ] {
            let mut source = source();
            source.grants.push(Grant {
                object: "lakekeeper_table:analytics/requests".into(),
                subject_kind: kind.into(),
                subject_id: id.into(),
                relation: relation.into(),
            });
            assert!(
                source.validate().is_err(),
                "invalid grant accepted: {kind}/{id}/{relation}"
            );
        }
    }

    #[test]
    fn legacy_source_rejects_cycles_and_invalid_parent_references() {
        let mut cyclic = source();
        cyclic.grants.push(Grant {
            object: "role:analysts".into(),
            subject_kind: "role".into(),
            subject_id: "nested".into(),
            relation: "assignee".into(),
        });
        assert!(cyclic.validate().is_err());
        for parent in ["namespace:foreign", "namespace:missing"] {
            let mut invalid = source();
            invalid
                .objects
                .get_mut("lakekeeper_table:analytics/requests")
                .unwrap()
                .parent = Some(parent.into());
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn permission_decision_requires_one_typed_boolean() {
        for body in [
            r#"{}"#,
            r#"[true]"#,
            r#"[]"#,
            r#"{"allowed":true,"resolution":42}"#,
            r#"{"allowed":true,"allowed":false}"#,
            r#"{"allowed":true,"error":"failed"}"#,
            r#"{"allowed":"true"}"#,
        ] {
            assert!(decode_object::<CheckDecision>(body.as_bytes()).is_err());
        }
        assert!(
            decode_object::<CheckDecision>(br#"{"allowed":true,"resolution":""}"#)
                .unwrap()
                .allowed
        );
    }

    #[test]
    fn migration_tuple_wire_format_accepts_only_unconditional_relationships() {
        assert!(decode_object::<ReadPage>(br#"{"error":"unavailable"}"#).is_err());
        assert!(decode_object::<ReadPage>(br#"{"tuples":[],"continuation_token":""}"#).is_ok());
        for value in [
            json!({"user":"user:alice","relation":"select","object":"lakekeeper_table:analytics/requests"}),
            json!({"user":"user:alice","relation":"select","object":"lakekeeper_table:analytics/requests","condition":null}),
        ] {
            let tuple: WireTupleKey = serde_json::from_value(value).unwrap();
            assert_eq!(tuple.unconditional().unwrap().user, "user:alice");
        }
        for condition in [
            json!({}),
            json!({"name":"conditional","context":{}}),
            json!(false),
        ] {
            let tuple: WireTupleKey=serde_json::from_value(json!({"user":"user:alice","relation":"select","object":"lakekeeper_table:analytics/requests","condition":condition})).unwrap();
            assert!(tuple.unconditional().is_err());
        }
    }

    #[test]
    fn model_comparison_preserves_policy_restrictions() {
        let model: Value = serde_json::from_slice(include_bytes!("../fga/model.json")).unwrap();
        let normalized = policy_model(&model).unwrap();
        let mut changed = model.clone();
        changed["type_definitions"][3]["relations"]["can_assume"] = json!({"this":{}});
        assert_ne!(normalized, policy_model(&changed).unwrap());
        assert_eq!(
            digest(include_bytes!("../fga/model.json")),
            "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
        );
    }
}
