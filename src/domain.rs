//! Pure domain decisions. Persistence and authorization are external effects.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Pending,
    Active,
    Deleting,
    Deleted,
}
impl Lifecycle {
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "deleting" => Some(Self::Deleting),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Provisioned,
    BeginDeletion,
    TuplesRemoved,
}

pub const fn transition(state: Lifecycle, event: Event) -> Option<Lifecycle> {
    match (state, event) {
        (Lifecycle::Pending, Event::Provisioned) => Some(Lifecycle::Active),
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

pub fn valid_task(title: &str, description: &str, status: &str) -> bool {
    !title.trim().is_empty()
        && title.len() <= 160
        && description.len() <= 4000
        && matches!(status, "todo" | "in_progress" | "done")
}

pub fn valid_idempotency_key(key: &str) -> bool {
    (8..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn valid_task_relation(relation: &str) -> bool {
    matches!(relation, "viewer" | "editor" | "assignee" | "blocked")
}

pub fn valid_organization_permission(relation: &str) -> bool {
    matches!(
        relation,
        "can_view_task" | "can_edit_task" | "can_delete_task"
    )
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
    fn validates_identifiers_and_input() {
        assert!(valid_id("access-review-123"));
        assert!(!valid_id("task:review"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"x".repeat(65)));
        assert!(!valid_task("", "", "todo"));
        assert!(valid_task(
            "Review access",
            "Evidence required",
            "in_progress"
        ));
        assert!(!valid_task("Review access", "", "active"));
        assert!(!valid_task_relation("assigner"));
        assert!(!valid_task_relation("can_delete"));
        assert!(!valid_organization_permission("owner"));
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    fn deletion_cannot_resurrect_or_be_visible() {
        let event = match kani::any::<u8>() % 3 {
            0 => Event::Provisioned,
            1 => Event::BeginDeletion,
            _ => Event::TuplesRemoved,
        };
        assert!(transition(Lifecycle::Deleted, event).is_none());
        assert!(!is_visible(Lifecycle::Deleting));
        assert!(!is_visible(Lifecycle::Deleted));
    }

    #[kani::proof]
    fn activation_requires_provisioning_pending_state() {
        let state = match kani::any::<u8>() % 4 {
            0 => Lifecycle::Pending,
            1 => Lifecycle::Active,
            2 => Lifecycle::Deleting,
            _ => Lifecycle::Deleted,
        };
        let event = match kani::any::<u8>() % 3 {
            0 => Event::Provisioned,
            1 => Event::BeginDeletion,
            _ => Event::TuplesRemoved,
        };
        if transition(state, event) == Some(Lifecycle::Active) {
            assert_eq!(state, Lifecycle::Pending);
            assert_eq!(event, Event::Provisioned);
        }
    }
}
