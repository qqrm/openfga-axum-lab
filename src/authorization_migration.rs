//! One-time, resumable SQL → OpenFGA migration.
//!
//! The source is an explicitly documented Openlane SQL contract, populated from
//! existing organization, membership and task relationships. It is frozen once a validated snapshot is bound to a
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

pub const PINNED_MODEL_SHA256: &str =
    "c9186020ca60a129a82e3408f122689669302a447f34c54e70ea006d794d59f4";

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
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) store_id: String,
    pub(crate) model_id: String,
    pub(crate) model_sha256: String,
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

/// A runtime gate independent of the mutable decision-cache revision.
/// A verified migration binding does not clear ambiguous-write debt.
pub async fn ensure_active(db: &PgPool, store: &str, model: &str, model_hash: &str) -> ToolResult {
    let active: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM openlane_authorization_migration WHERE singleton AND status='active' AND store_id=$1 AND model_id=$2 AND model_sha256=$3)")
        .bind(store).bind(model).bind(model_hash).fetch_one(db).await?;
    if !active {
        return Err(
            "Openlane authorization migration is not active for the configured target".into(),
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
struct Object {
    kind: String,
    id: String,
    organization: String,
    owner: Option<String>,
    assigner: Option<String>,
    parent: Option<String>,
}

impl Object {
    fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Membership {
    organization: String,
    user: String,
    role: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Grant {
    object: String,
    user: String,
    relation: String,
}

#[derive(Clone, Debug, Serialize)]
struct Source {
    users: Vec<String>,
    memberships: BTreeSet<Membership>,
    objects: BTreeMap<String, Object>,
    grants: BTreeSet<Grant>,
}

impl Source {
    async fn read(connection: &mut sqlx::PgConnection) -> ToolResult<Self> {
        let incompatible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_projects) OR EXISTS(SELECT 1 FROM legacy_resource_grants) OR EXISTS(SELECT 1 FROM authorization_migration WHERE source_sha256 IS NOT NULL)")
            .fetch_one(&mut *connection).await?;
        if incompatible {
            return Err("Lakekeeper data/binding is incompatible; use an isolated Openlane database and an explicitly reviewed source import".into());
        }
        let unresolved: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM authorization_jobs) OR EXISTS(SELECT 1 FROM organizations WHERE authz_pending OR authz_uncertain)")
            .fetch_one(&mut *connection).await?;
        if unresolved {
            return Err(
                "resolve existing authorization mutations before the initial Openlane cutover"
                    .into(),
            );
        }
        let users: Vec<String> = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 10001")
            .fetch_all(&mut *connection)
            .await?;
        let memberships = sqlx::query_as::<_,(String,String,String)>("SELECT organization_id,user_id,role FROM organization_members ORDER BY organization_id,user_id LIMIT 10001")
            .fetch_all(&mut *connection).await?.into_iter()
            .map(|(organization,user,role)| Membership {organization,user,role}).collect();
        let mut objects = BTreeMap::new();
        for (id, owner) in sqlx::query_as::<_, (String, String)>(
            "SELECT id,owner_id FROM organizations ORDER BY id LIMIT 10001",
        )
        .fetch_all(&mut *connection)
        .await?
        {
            let object = Object {
                kind: "organization".into(),
                organization: id.clone(),
                id,
                owner: Some(owner),
                assigner: None,
                parent: None,
            };
            objects.insert(object.key(), object);
        }
        for (id, organization, assigner, status) in
            sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT id,organization_id,assigner_id,status FROM tasks ORDER BY id LIMIT 10001",
            )
            .fetch_all(&mut *connection)
            .await?
        {
            if status != "active" {
                return Err("source contains an unresolved or deleted task; resolve lifecycle state before initial cutover".into());
            }
            let object = Object {
                kind: "task".into(),
                id,
                parent: Some(format!("organization:{organization}")),
                organization,
                owner: None,
                assigner: Some(assigner),
            };
            objects.insert(object.key(), object);
        }
        let mut grants = BTreeSet::new();
        for (organization,user,relation) in sqlx::query_as::<_,(String,String,String)>(
            "SELECT organization_id,user_id,relation FROM organization_permissions ORDER BY organization_id,user_id,relation LIMIT 10001")
            .fetch_all(&mut *connection).await? {
            grants.insert(Grant {object:format!("organization:{organization}"),user,relation});
        }
        for (task,user,relation) in sqlx::query_as::<_,(String,String,String)>(
            "SELECT task_id,user_id,relation FROM task_relations ORDER BY task_id,user_id,relation LIMIT 10001")
            .fetch_all(&mut *connection).await? {
            grants.insert(Grant {object:format!("task:{task}"),user,relation});
        }
        Ok(Self {
            users,
            memberships,
            objects,
            grants,
        })
    }

    fn validate(&self) -> ToolResult {
        let rows =
            self.users.len() + self.memberships.len() + self.objects.len() + self.grants.len();
        if rows > MAX_SOURCE_ROWS || self.objects.is_empty() || self.users.is_empty() {
            return Err(
                "Openlane authorization source is empty or exceeds the bounded migration size"
                    .into(),
            );
        }
        if self.users.iter().any(|user| !identifier(user))
            || self.users.iter().collect::<BTreeSet<_>>().len() != self.users.len()
        {
            return Err("invalid or duplicate source user identifier".into());
        }
        let known_user =
            |user: &str| identifier(user) && self.users.iter().any(|known| known == user);
        let mut membership_keys = BTreeSet::new();
        for membership in &self.memberships {
            if !known_user(&membership.user)
                || !self
                    .objects
                    .contains_key(&format!("organization:{}", membership.organization))
                || !matches!(membership.role.as_str(), "member" | "admin")
                || !membership_keys.insert((&membership.organization, &membership.user))
            {
                return Err("invalid organization membership or conflicting role".into());
            }
        }
        for (key, object) in &self.objects {
            if key != &object.key() || !identifier(&object.id) || !identifier(&object.organization)
            {
                return Err("invalid source object identifier".into());
            }
            match object.kind.as_str() {
                "organization" => {
                    let owner = object.owner.as_deref().ok_or("organization has no owner")?;
                    if object.organization != object.id
                        || object.parent.is_some()
                        || object.assigner.is_some()
                        || !known_user(owner)
                        || !self
                            .memberships
                            .iter()
                            .any(|m| m.organization == object.id && m.user == owner)
                    {
                        return Err(
                            "organization owner must be an existing member of that organization"
                                .into(),
                        );
                    }
                }
                "task" => {
                    let assigner = object.assigner.as_deref().ok_or("task has no assigner")?;
                    let parent = format!("organization:{}", object.organization);
                    if object.owner.is_some()
                        || !known_user(assigner)
                        || object.parent.as_deref() != Some(&parent)
                        || !self
                            .objects
                            .get(&parent)
                            .is_some_and(|org| org.kind == "organization")
                    {
                        return Err("task has an invalid assigner or organization parent".into());
                    }
                }
                _ => return Err("unsupported source object kind".into()),
            }
        }
        for grant in &self.grants {
            let object = self
                .objects
                .get(&grant.object)
                .ok_or("grant targets a missing object")?;
            if !known_user(&grant.user) {
                return Err(
                    "unknown, wildcard or userset subjects are outside the source contract".into(),
                );
            }
            let allowed = match object.kind.as_str() {
                "organization" => matches!(
                    grant.relation.as_str(),
                    "can_view_task" | "can_edit_task" | "can_delete_task"
                ),
                "task" => matches!(
                    grant.relation.as_str(),
                    "viewer" | "editor" | "assignee" | "blocked"
                ),
                _ => false,
            };
            if !allowed {
                return Err("unsupported or escalating source relation".into());
            }
            // Explicit cross-organization user grants are a supported Openlane
            // policy. They do not create membership or an organization owner.
        }
        Ok(())
    }

    fn tuples(&self) -> ToolResult<BTreeSet<Tuple>> {
        let mut tuples = BTreeSet::new();
        for object in self.objects.values() {
            if let Some(owner) = &object.owner {
                tuples.insert(Tuple {
                    user: format!("user:{owner}"),
                    relation: "owner".into(),
                    object: object.key(),
                });
            }
            if let Some(assigner) = &object.assigner {
                tuples.insert(Tuple {
                    user: format!("user:{assigner}"),
                    relation: "assigner".into(),
                    object: object.key(),
                });
            }
            if let Some(parent) = &object.parent {
                tuples.insert(Tuple {
                    user: parent.clone(),
                    relation: "parent_context".into(),
                    object: object.key(),
                });
            }
        }
        for member in &self.memberships {
            tuples.insert(Tuple {
                user: format!("user:{}", member.user),
                relation: member.role.clone(),
                object: format!("organization:{}", member.organization),
            });
        }
        for grant in &self.grants {
            tuples.insert(Tuple {
                user: format!("user:{}", grant.user),
                relation: grant.relation.clone(),
                object: grant.object.clone(),
            });
        }
        if tuples.len() > MAX_SOURCE_ROWS {
            return Err("tuple plan exceeds supported migration size".into());
        }
        Ok(tuples)
    }

    fn direct(&self, user: &str, object: &Object, relation: &str) -> bool {
        self.grants.contains(&Grant {
            object: object.key(),
            user: user.into(),
            relation: relation.into(),
        })
    }

    /// Independent SQL authorization evaluator. It uses source rows, never
    /// generated tuples or the OpenFGA model's expression evaluator.
    fn source_check(&self, user: &str, object: &Object, action: &str) -> ToolResult<bool> {
        if !self.users.iter().any(|known| known == user) {
            return Ok(false);
        }
        match object.kind.as_str() {
            "organization" => {
                let owner = object.owner.as_deref() == Some(user);
                let role = self
                    .memberships
                    .iter()
                    .find(|m| m.organization == object.id && m.user == user)
                    .map(|m| m.role.as_str());
                let admin = role == Some("admin");
                let member = role == Some("member") || owner || admin;
                let delete_task = owner || self.direct(user, object, "can_delete_task");
                let edit_task = delete_task || self.direct(user, object, "can_edit_task");
                let view_task = edit_task || self.direct(user, object, "can_view_task");
                Ok(match action {
                    "owner" | "full_access" | "can_delete" => owner,
                    "admin" => admin,
                    "member" | "can_view" => member,
                    "can_edit" => owner || admin,
                    "can_delete_task" => delete_task,
                    "can_edit_task" => edit_task,
                    "can_view_task" => view_task,
                    "can_create_task" => owner || admin || edit_task,
                    _ => return Err("unsupported organization check".into()),
                })
            }
            "task" => {
                let parent = self
                    .objects
                    .get(object.parent.as_deref().ok_or("missing task parent")?)
                    .ok_or("missing organization")?;
                let parent_view = self.source_check(user, parent, "can_view_task")?;
                let parent_edit = self.source_check(user, parent, "can_edit_task")?;
                let parent_delete = self.source_check(user, parent, "can_delete_task")?;
                let assigner = object.assigner.as_deref() == Some(user);
                let assignee = self.direct(user, object, "assignee");
                let blocked = self.direct(user, object, "blocked");
                let deleter = assigner || parent_edit || parent_delete;
                let editor =
                    assigner || assignee || parent_edit || self.direct(user, object, "editor");
                let viewer = editor || parent_view || self.direct(user, object, "viewer");
                Ok(match action {
                    "assigner" => assigner,
                    "assignee" => assignee,
                    "blocked" => blocked,
                    "parent_viewer" => parent_view,
                    "parent_editor" => parent_edit,
                    "parent_deleter" => parent_delete,
                    "deleter" | "can_delete" => deleter,
                    "editor" => editor,
                    "viewer" => viewer,
                    "can_view" => viewer && !blocked,
                    "can_edit" => (editor && !blocked) || deleter,
                    _ => return Err("unsupported task check".into()),
                })
            }
            _ => Err("unsupported source object kind".into()),
        }
    }
}

fn actions(kind: &str) -> &'static [&'static str] {
    match kind {
        "organization" => &[
            "owner",
            "admin",
            "member",
            "full_access",
            "can_delete",
            "can_edit",
            "can_view",
            "can_view_task",
            "can_edit_task",
            "can_delete_task",
            "can_create_task",
        ],
        "task" => &[
            "assigner",
            "assignee",
            "parent_viewer",
            "parent_editor",
            "parent_deleter",
            "deleter",
            "editor",
            "viewer",
            "blocked",
            "can_view",
            "can_edit",
            "can_delete",
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

const SOURCE_TABLES: &str = "LOCK TABLE users, organizations, organization_members, organization_permissions, tasks, task_relations IN SHARE MODE";

/// Validate the immutable local contract before connecting to SQL or seeding.
/// Runtime, provisioning and migration use the same reviewed model hash.
pub(crate) fn validate_configuration(
    config_path: &Path,
    model_path: &Path,
) -> ToolResult<(Target, Value)> {
    let target: Target = serde_json::from_slice(&fs::read(config_path)?)?;
    let valid_id = |id: &str| {
        id.len() == 26
            && id
                .bytes()
                .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
    };
    if !valid_id(&target.store_id) || !valid_id(&target.model_id) {
        return Err("invalid OpenFGA target identifiers".into());
    }
    let bytes = fs::read(model_path)?;
    if target.model_sha256 != digest(&bytes) || target.model_sha256 != PINNED_MODEL_SHA256 {
        return Err("model hash differs from the pinned Openlane migration contract".into());
    }
    let model: Value = serde_json::from_slice(&bytes)?;
    policy_model(&model)?;
    Ok((target, model))
}

fn backend(base: &str, target: Target) -> ToolResult<Backend> {
    let parsed = reqwest::Url::parse(base)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(
            "FGA HTTP endpoint must be an HTTP(S) URL without credentials, query or fragment"
                .into(),
        );
    }
    Ok(Backend {
        client: Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()?,
        base: base.trim_end_matches('/').into(),
        target,
    })
}

pub(crate) async fn validate_backend(base: &str, target: &Target, model: &Value) -> ToolResult {
    backend(base, target.clone())?.validate_target(model).await
}

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
    let (target, expected_model) = validate_configuration(config_path, model_path)?;
    let backend = backend(base, target)?;
    // Transaction-scoped lock releases even if this process is cancelled. It
    // is deliberately independent from the durable checkpoints below.
    let mut run_lock = db.begin().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(6041927238613302)")
        .fetch_one(&mut *run_lock)
        .await?;
    if !acquired {
        return Err("another authorization migration is already running".into());
    }
    backend.validate_target(&expected_model).await?;
    let mut snapshot = db.begin().await?;
    sqlx::query(SOURCE_TABLES).execute(&mut *snapshot).await?;
    let row=sqlx::query("SELECT status,store_id,model_id,model_sha256,source_sha256,source_json,next_tuple,tuple_count,verified_checks,attempted FROM openlane_authorization_migration WHERE singleton FOR UPDATE").fetch_one(&mut *snapshot).await?;
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
    let source_json = serde_json::to_string(&source)?;
    let source_hash = digest(source_json.as_bytes());
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
        return Err("Openlane permission matrix exceeds supported migration bound".into());
    }
    let mut next = usize::try_from(row.try_get::<i64, _>("next_tuple")?)?;
    if status == "pending" {
        if !backend.tuples().await?.is_empty() {
            return Err("initial migration requires an empty dedicated OpenFGA store".into());
        }
        sqlx::query("UPDATE openlane_authorization_migration SET status='writing',store_id=$1,model_id=$2,model_sha256=$3,source_sha256=$4,tuple_count=$5,source_json=$6,next_tuple=0,verified_checks=0,attempted=false,updated_at=now() WHERE singleton")
            .bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).bind(&source_hash).bind(i64::try_from(tuples.len())?).bind(&source_json).execute(&mut *snapshot).await?;
        next = 0;
    } else if row
        .try_get::<Option<String>, _>("source_sha256")?
        .as_deref()
        != Some(&source_hash)
        || row.try_get::<Option<String>, _>("source_json")?.as_deref() != Some(source_json.as_str())
        || usize::try_from(row.try_get::<i64, _>("tuple_count")?)? != tuples.len()
        || next > tuples.len()
    {
        return Err("Openlane source or tuple plan differs from the persisted checkpoint".into());
    }
    if row.try_get::<bool, _>("attempted")? {
        // A prior writer may still be buffered before its OpenFGA commit.
        // Idempotent replay proves the current graph, not remote quiescence.
        // Persist independent global debt BEFORE replay/activation; ordinary
        // resume cannot clear it. Only the explicit drained-recovery command
        // may make the verified target available for requests again.
        sqlx::query("UPDATE authz_cache_state SET pending=pending+1,generation=generation+1 WHERE singleton")
            .execute(&mut *snapshot).await?;
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
        let marked = sqlx::query("UPDATE openlane_authorization_migration SET attempted=true,updated_at=now() WHERE singleton AND source_sha256=$1 AND store_id=$2 AND model_id=$3 AND model_sha256=$4 AND next_tuple=$5 AND status IN ('writing','verifying')")
            .bind(&source_hash).bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).bind(i64::try_from(next)?).execute(db).await?.rows_affected();
        if marked != 1 {
            return Err("migration attempt binding changed before remote write".into());
        }
        backend.write(&tuples[next..end]).await?;
        let advanced = sqlx::query("UPDATE openlane_authorization_migration SET next_tuple=$1,status='writing',attempted=false,updated_at=now() WHERE singleton AND source_sha256=$2 AND store_id=$3 AND model_id=$4 AND model_sha256=$5 AND next_tuple=$6 AND status IN ('writing','verifying')")
            .bind(i64::try_from(end)?).bind(&source_hash).bind(&backend.target.store_id).bind(&backend.target.model_id).bind(&backend.target.model_sha256).bind(i64::try_from(next)?).execute(db).await?.rows_affected();
        if advanced != 1 {
            return Err(
                "migration checkpoint binding changed after remote write; access remains inactive"
                    .into(),
            );
        }
        next = end;
    }
    let verifying = sqlx::query("UPDATE openlane_authorization_migration SET status='verifying',verified_checks=0,updated_at=now() WHERE singleton AND status IN ('writing','verifying') AND source_sha256=$1 AND store_id=$2 AND model_id=$3 AND model_sha256=$4 AND next_tuple=tuple_count AND NOT attempted")
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
                let expected = source.source_check(user, object, action)?;
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
        return Err("Openlane source changed during permission verification".into());
    }
    let changed=sqlx::query("UPDATE openlane_authorization_migration SET status='active',verified_checks=$1,activated_at=now(),updated_at=now() WHERE singleton AND status='verifying' AND source_sha256=$2 AND store_id=$3 AND model_id=$4 AND model_sha256=$5 AND next_tuple=tuple_count AND NOT attempted")
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
    let base = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
    let config = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let model = std::env::var("FGA_MODEL").unwrap_or_else(|_| "/app/fga/model.json".into());
    let batch = std::env::var("AUTHZ_MIGRATION_BATCH_SIZE")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(MAX_BATCH_SIZE);
    if !(1..=MAX_BATCH_SIZE).contains(&batch) {
        return Err("AUTHZ_MIGRATION_BATCH_SIZE must be between 1 and 100".into());
    }
    let (target, expected_model) = validate_configuration(Path::new(&config), Path::new(&model))?;
    validate_backend(&base, &target, &expected_model).await?;
    let database = crate::configuration::secret("DATABASE_URL")?;
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
        for (kind, id, organization, owner, assigner, parent) in [
            ("organization", "acme", "acme", Some("alice"), None, None),
            ("organization", "other", "other", Some("erin"), None, None),
            (
                "task",
                "access-review",
                "acme",
                None,
                Some("carol"),
                Some("organization:acme"),
            ),
            (
                "task",
                "vendor-review",
                "acme",
                None,
                Some("alice"),
                Some("organization:acme"),
            ),
            (
                "task",
                "other-review",
                "other",
                None,
                Some("erin"),
                Some("organization:other"),
            ),
        ] {
            let object = Object {
                kind: kind.into(),
                id: id.into(),
                organization: organization.into(),
                owner: owner.map(str::to_owned),
                assigner: assigner.map(str::to_owned),
                parent: parent.map(str::to_owned),
            };
            objects.insert(object.key(), object);
        }
        Source {
            users: ["alice", "bob", "carol", "dave", "erin"]
                .map(str::to_owned)
                .to_vec(),
            memberships: [
                ("acme", "alice", "member"),
                ("acme", "bob", "admin"),
                ("acme", "carol", "member"),
                ("other", "erin", "member"),
                ("other", "dave", "member"),
            ]
            .into_iter()
            .map(|(organization, user, role)| Membership {
                organization: organization.into(),
                user: user.into(),
                role: role.into(),
            })
            .collect(),
            objects,
            grants: BTreeSet::from([
                Grant {
                    object: "organization:acme".into(),
                    user: "bob".into(),
                    relation: "can_view_task".into(),
                },
                Grant {
                    object: "task:vendor-review".into(),
                    user: "dave".into(),
                    relation: "viewer".into(),
                },
            ]),
        }
    }

    #[test]
    fn sql_owners_roles_inheritance_and_direct_sharing_preserve_privilege_tiers() {
        let source = source();
        source.validate().unwrap();
        let task = &source.objects["task:access-review"];
        assert!(source.source_check("bob", task, "can_view").unwrap());
        assert!(!source.source_check("bob", task, "can_edit").unwrap());
        assert!(!source.source_check("bob", task, "can_delete").unwrap());
        assert!(source.source_check("carol", task, "can_delete").unwrap());
        assert!(source.source_check("alice", task, "can_delete").unwrap());
        assert!(
            source
                .source_check(
                    "bob",
                    &source.objects["organization:acme"],
                    "can_create_task"
                )
                .unwrap()
        );
        for action in actions("task") {
            assert!(!source.source_check("erin", task, action).unwrap());
            assert!(!source.source_check("unknown", task, action).unwrap());
        }
        assert!(
            source
                .source_check("dave", &source.objects["task:vendor-review"], "can_view")
                .unwrap()
        );
        let tuples = source.tuples().unwrap();
        for (user, relation, object) in [
            ("user:alice", "owner", "organization:acme"),
            ("organization:acme", "parent_context", "task:access-review"),
            ("user:carol", "assigner", "task:access-review"),
            ("user:bob", "admin", "organization:acme"),
        ] {
            assert!(tuples.contains(&Tuple {
                user: user.into(),
                relation: relation.into(),
                object: object.into()
            }));
        }
    }

    #[test]
    fn blocked_precedence_matches_upstream_and_does_not_hide_delete_permission() {
        let mut source = source();
        source.grants.insert(Grant {
            object: "task:access-review".into(),
            user: "carol".into(),
            relation: "blocked".into(),
        });
        source.grants.insert(Grant {
            object: "task:access-review".into(),
            user: "bob".into(),
            relation: "editor".into(),
        });
        source.grants.insert(Grant {
            object: "task:access-review".into(),
            user: "bob".into(),
            relation: "blocked".into(),
        });
        let task = &source.objects["task:access-review"];
        assert!(!source.source_check("carol", task, "can_view").unwrap());
        assert!(source.source_check("carol", task, "can_edit").unwrap());
        assert!(source.source_check("carol", task, "can_delete").unwrap());
        assert!(!source.source_check("bob", task, "can_view").unwrap());
        assert!(!source.source_check("bob", task, "can_edit").unwrap());
        assert!(!source.source_check("bob", task, "can_delete").unwrap());
    }

    #[test]
    fn sql_source_rejects_unknown_users_usersets_wildcards_and_derived_grants() {
        for (object, user, relation) in [
            ("task:access-review", "missing", "viewer"),
            ("task:access-review", "*", "viewer"),
            ("task:access-review", "bob#member", "viewer"),
            ("task:access-review", "bob", "can_delete"),
            ("task:access-review", "bob", "assigner"),
            ("organization:acme", "bob", "owner"),
        ] {
            let mut source = source();
            source.grants.insert(Grant {
                object: object.into(),
                user: user.into(),
                relation: relation.into(),
            });
            assert!(
                source.validate().is_err(),
                "invalid grant accepted: {object}/{user}/{relation}"
            );
        }
    }

    #[test]
    fn sql_source_rejects_inconsistent_parent_owner_and_membership_roles() {
        for parent in ["organization:other", "organization:missing"] {
            let mut source = source();
            source.objects.get_mut("task:access-review").unwrap().parent = Some(parent.into());
            assert!(source.validate().is_err());
        }
        let mut missing_member = source();
        missing_member
            .memberships
            .retain(|member| member.user != "alice");
        assert!(missing_member.validate().is_err());
        let mut duplicate_role = source();
        duplicate_role.memberships.insert(Membership {
            organization: "acme".into(),
            user: "bob".into(),
            role: "member".into(),
        });
        assert!(duplicate_role.validate().is_err());
        let mut escalating_role = source();
        escalating_role.memberships.insert(Membership {
            organization: "acme".into(),
            user: "dave".into(),
            role: "owner".into(),
        });
        assert!(escalating_role.validate().is_err());
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
            json!({"user":"user:alice","relation":"viewer","object":"task:access-review"}),
            json!({"user":"user:alice","relation":"viewer","object":"task:access-review","condition":null}),
        ] {
            let tuple: WireTupleKey = serde_json::from_value(value).unwrap();
            assert_eq!(tuple.unconditional().unwrap().user, "user:alice");
        }
        for condition in [
            json!({}),
            json!({"name":"conditional","context":{}}),
            json!(false),
        ] {
            let tuple: WireTupleKey=serde_json::from_value(json!({"user":"user:alice","relation":"viewer","object":"task:access-review","condition":condition})).unwrap();
            assert!(tuple.unconditional().is_err());
        }
    }

    #[test]
    fn model_comparison_preserves_policy_restrictions() {
        let model: Value = serde_json::from_slice(include_bytes!("../fga/model.json")).unwrap();
        let normalized = policy_model(&model).unwrap();
        let mut changed = model.clone();
        changed["type_definitions"][2]["relations"]["can_view"] = json!({"this":{}});
        assert_ne!(normalized, policy_model(&changed).unwrap());
        assert_eq!(
            digest(include_bytes!("../fga/model.json")),
            PINNED_MODEL_SHA256
        );
    }
}
