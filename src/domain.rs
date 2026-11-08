//! Pure domain decisions. Persistence and authorization are external effects.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Pending,
    Active,
    Updating,
    Deleting,
    Deleted,
}
impl Lifecycle {
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
pub enum Event {
    Provisioned,
    BeginUpdate,
    Updated,
    BeginDeletion,
    TuplesRemoved,
}

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

pub const fn is_visible(state: Lifecycle) -> bool {
    matches!(state, Lifecycle::Active)
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn valid_table(name: &str, description: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 100 && description.len() <= 2000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        pub struct $name(String);
        impl $name {
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
pub struct Scope {
    pub project: ProjectId,
    pub warehouse: WarehouseId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    User(UserId),
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
pub enum Object {
    Table { warehouse: WarehouseId, id: TableId },
    Namespace(NamespaceId),
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
pub enum Privilege {
    Describe,
    Select,
    Write,
    Modify,
    PassGrants,
    ManageGrants,
}
impl Privilege {
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
pub enum TableAction {
    Metadata,
    ReadData,
    Commit,
    Drop,
    ReadAssignments,
    ChangeOwnership,
    Move,
}
impl TableAction {
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
