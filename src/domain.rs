//! Pure domain decisions. Persistence and authorization are external effects.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Persisted lifecycle of a catalog table.
pub enum Lifecycle {
    /// SQL exists, but initial authorization tuples are not yet confirmed.
    Pending,
    /// The table is eligible for an authorization check and data access.
    Active,
    /// Ownership or hierarchy replacement is not yet fully acknowledged.
    Updating,
    /// Deletion has started; the table remains hidden until cleanup completes.
    Deleting,
    /// A permanent tombstone prevents identifier reuse and resurrection.
    Deleted,
}
impl Lifecycle {
    /// Parse a persisted state, refusing unknown values.
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "updating" => Some(Self::Updating),
            "deleting" => Some(Self::Deleting),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Confirmed events that may advance a table's lifecycle.
pub enum Event {
    /// Initial authorization tuples were acknowledged.
    Provisioned,
    /// A durable ownership or hierarchy replacement was staged.
    BeginUpdate,
    /// The relationship replacement and SQL projection were confirmed.
    Updated,
    /// An authorized deletion was durably staged.
    BeginDeletion,
    /// The table's authorization relationships were removed.
    TuplesRemoved,
}

/// Return the next valid state; reject skips, repetitions and resurrection.
pub const fn transition(state: Lifecycle, event: Event) -> Option<Lifecycle> {
    match (state, event) {
        (Lifecycle::Pending, Event::Provisioned) => Some(Lifecycle::Active),
        (Lifecycle::Active, Event::BeginUpdate) => Some(Lifecycle::Updating),
        (Lifecycle::Updating, Event::Updated) => Some(Lifecycle::Active),
        (Lifecycle::Active, Event::BeginDeletion) => Some(Lifecycle::Deleting),
        (Lifecycle::Deleting, Event::TuplesRemoved) => Some(Lifecycle::Deleted),
        _ => None,
    }
}

/// Only fully active tables may proceed to object authorization.
pub const fn is_visible(state: Lifecycle) -> bool {
    matches!(state, Lifecycle::Active)
}

/// Accept a nonempty ASCII identifier of at most 64 bytes, without FGA syntax.
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Validate a nonblank name (100 UTF-8 bytes) and description (2,000 bytes).
pub fn valid_table(name: &str, description: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 100 && description.len() <= 2000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// An identifier failed the domain grammar.
pub struct InvalidId;
impl std::fmt::Display for InvalidId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid identifier")
    }
}
impl std::error::Error for InvalidId {}

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[doc = concat!("Validated ", stringify!($name), " identifier without an OpenFGA type prefix.")]
        pub struct $name(String);
        impl $name {
            /// Borrow the validated identifier text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = InvalidId;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                valid_id(&value).then_some(Self(value)).ok_or(InvalidId)
            }
        }
        impl TryFrom<&str> for $name {
            type Error = InvalidId;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::try_from(value.to_owned())
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}
identifier!(UserId);
identifier!(ProjectId);
identifier!(WarehouseId);
identifier!(NamespaceId);
identifier!(TableId);
identifier!(RoleId);

#[derive(Clone, Debug, PartialEq, Eq)]
/// SQL tenant and warehouse scope for all catalog table operations.
pub struct Scope {
    /// Project membership defines the application tenant boundary.
    pub project: ProjectId,
    /// Warehouse selected within that project.
    pub warehouse: WarehouseId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// A direct user or a role's effective assignees.
pub enum Subject {
    /// A known user in the scoped project.
    User(UserId),
    /// Assignees of a role in the scoped project.
    Role(RoleId),
}
impl std::fmt::Display for Subject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(id) => write!(formatter, "user:{id}"),
            Self::Role(id) => write!(formatter, "role:{id}#assignee"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Supported catalog object keys in the pinned authorization model.
pub enum Object {
    /// A table identifier is qualified by its warehouse.
    Table {
        /// Warehouse containing the table.
        warehouse: WarehouseId,
        /// Table identifier within the warehouse.
        id: TableId,
    },
    /// Namespace hierarchy and inherited permissions.
    Namespace(NamespaceId),
    /// Project-scoped role and its assignees.
    Role(RoleId),
}
impl std::fmt::Display for Object {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Table { warehouse, id } => write!(formatter, "lakekeeper_table:{warehouse}/{id}"),
            Self::Namespace(id) => write!(formatter, "namespace:{id}"),
            Self::Role(id) => write!(formatter, "role:{id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Direct privileges exposed by the catalog API.
pub enum Privilege {
    /// Inspect metadata.
    Describe,
    /// Read table data.
    Select,
    /// Write table data.
    Write,
    /// Modify the table's structure and lifecycle.
    Modify,
    /// Delegate permitted data privileges.
    PassGrants,
    /// Manage grants according to the upstream model.
    ManageGrants,
}
impl Privilege {
    /// Parse an exposed privilege; derived relations are intentionally rejected.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "describe" => Some(Self::Describe),
            "select" => Some(Self::Select),
            "write" => Some(Self::Write),
            "modify" => Some(Self::Modify),
            "pass_grants" => Some(Self::PassGrants),
            "manage_grants" => Some(Self::ManageGrants),
            _ => None,
        }
    }
    /// Return the direct relation name in the pinned model.
    pub const fn relation(self) -> &'static str {
        match self {
            Self::Describe => "describe",
            Self::Select => "select",
            Self::Write => "write",
            Self::Modify => "modify",
            Self::PassGrants => "pass_grants",
            Self::ManageGrants => "manage_grants",
        }
    }
    /// Return the permission required to grant this privilege.
    pub const fn grant_action(self) -> &'static str {
        match self {
            Self::Describe => "can_grant_describe",
            Self::Select => "can_grant_select",
            Self::Write => "can_grant_write",
            Self::Modify => "can_grant_modify",
            Self::PassGrants => "can_grant_pass_grants",
            Self::ManageGrants => "can_grant_manage_grants",
        }
    }
    /// Return the permission required to revoke this privilege.
    pub const fn revoke_action(self) -> &'static str {
        match self {
            Self::Describe => "can_revoke_describe",
            Self::Select => "can_revoke_select",
            Self::Write => "can_revoke_write",
            Self::Modify => "can_revoke_modify",
            // These privileges are management-only in both directions upstream.
            Self::PassGrants => "can_grant_pass_grants",
            Self::ManageGrants => "can_grant_manage_grants",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Effective permissions exposed by the catalog's Check endpoint.
pub enum TableAction {
    /// Read table metadata.
    Metadata,
    /// Read table data.
    ReadData,
    /// Commit table changes.
    Commit,
    /// Delete the table.
    Drop,
    /// Inspect direct assignments.
    ReadAssignments,
    /// Replace ownership using the model's delegation policy.
    ChangeOwnership,
    /// Move the table, also requiring permission at the destination.
    Move,
}
impl TableAction {
    /// Parse one of the supported effective actions.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "can_get_metadata" => Some(Self::Metadata),
            "can_read_data" => Some(Self::ReadData),
            "can_commit" => Some(Self::Commit),
            "can_drop" => Some(Self::Drop),
            "can_read_assignments" => Some(Self::ReadAssignments),
            "can_change_ownership" => Some(Self::ChangeOwnership),
            "can_move" => Some(Self::Move),
            _ => None,
        }
    }
    /// Return the effective permission name in the pinned model.
    pub const fn relation(self) -> &'static str {
        match self {
            Self::Metadata => "can_get_metadata",
            Self::ReadData => "can_read_data",
            Self::Commit => "can_commit",
            Self::Drop => "can_drop",
            Self::ReadAssignments => "can_read_assignments",
            Self::ChangeOwnership => "can_change_ownership",
            Self::Move => "can_move",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rejects_skips_and_resurrection() {
        assert_eq!(
            transition(Lifecycle::Pending, Event::Provisioned),
            Some(Lifecycle::Active)
        );
        assert_eq!(transition(Lifecycle::Active, Event::TuplesRemoved), None);
        assert_eq!(transition(Lifecycle::Deleted, Event::Provisioned), None);
        assert!(!is_visible(Lifecycle::Deleting));
    }
    #[test]
    fn relationship_replacement_stays_hidden_until_confirmed() {
        assert_eq!(
            transition(Lifecycle::Active, Event::BeginUpdate),
            Some(Lifecycle::Updating)
        );
        assert!(!is_visible(Lifecycle::Updating));
        assert_eq!(
            transition(Lifecycle::Updating, Event::Updated),
            Some(Lifecycle::Active)
        );
        assert_eq!(transition(Lifecycle::Pending, Event::Updated), None);
        assert_eq!(transition(Lifecycle::Deleted, Event::Updated), None);
    }
    #[test]
    fn validates_identifiers_and_input() {
        assert!(valid_id("requests-123"));
        assert!(!valid_id("table:requests"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"x".repeat(65)));
        assert!(!valid_table("", ""));
        assert!(valid_table("requests", "HTTP requests aggregate"));
        assert!(UserId::try_from("user:alice").is_err());
        assert!(NamespaceId::try_from("../other").is_err());
        assert_eq!(Privilege::Select.revoke_action(), "can_revoke_select");
        assert!(Privilege::parse("ownership").is_none());
        assert!(TableAction::parse("admin").is_none());
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    fn deletion_cannot_resurrect_or_be_visible() {
        let event = match kani::any::<u8>() % 5 {
            0 => Event::Provisioned,
            1 => Event::BeginDeletion,
            2 => Event::TuplesRemoved,
            3 => Event::BeginUpdate,
            _ => Event::Updated,
        };
        assert!(transition(Lifecycle::Deleted, event).is_none());
        assert!(!is_visible(Lifecycle::Deleting));
        assert!(!is_visible(Lifecycle::Deleted));
        assert!(!is_visible(Lifecycle::Updating));
    }

    #[kani::proof]
    fn activation_requires_confirmed_transition() {
        let state = match kani::any::<u8>() % 5 {
            0 => Lifecycle::Pending,
            1 => Lifecycle::Active,
            2 => Lifecycle::Deleting,
            3 => Lifecycle::Deleted,
            _ => Lifecycle::Updating,
        };
        let event = match kani::any::<u8>() % 5 {
            0 => Event::Provisioned,
            1 => Event::BeginDeletion,
            2 => Event::TuplesRemoved,
            3 => Event::BeginUpdate,
            _ => Event::Updated,
        };
        if transition(state, event) == Some(Lifecycle::Active) {
            assert!(
                (state == Lifecycle::Pending && event == Event::Provisioned)
                    || (state == Lifecycle::Updating && event == Event::Updated)
            );
        }
    }
}
