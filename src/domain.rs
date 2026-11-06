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

pub fn valid_pet(name: &str, species: &str, biography: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= 100
        && !species.trim().is_empty()
        && species.len() <= 50
        && biography.len() <= 2000
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
        assert!(valid_id("fox-123"));
        assert!(!valid_id("pet:fox"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"x".repeat(65)));
        assert!(!valid_pet("", "fox", ""));
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
