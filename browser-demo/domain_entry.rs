//! Scalar WebAssembly interface to the production domain rules.
//!
//! This dependency-free package shares the server's domain source. It does not
//! start Axum or replace real authorization with browser-side decisions.

#[path = "../src/domain.rs"]
#[allow(dead_code)]
mod domain;

/// Returns `1` only for active state `1`; unknown states return `0`.
///
/// ```
/// use openfga_axum_browser_demo::is_visible;
///
/// assert_eq!(is_visible(0), 0);
/// assert_eq!(is_visible(1), 1);
/// assert_eq!(is_visible(3), 0);
/// assert_eq!(is_visible(u32::MAX), 0);
/// ```
// SAFETY: this cdylib owns the two documented export names; neither export
// accepts pointers or depends on a caller-maintained memory invariant.
#[unsafe(no_mangle)]
pub extern "C" fn is_visible(state: u32) -> u32 {
    let state = match state {
        0 => domain::Lifecycle::Pending,
        1 => domain::Lifecycle::Active,
        2 => domain::Lifecycle::Deleting,
        3 => domain::Lifecycle::Deleted,
        _ => return 0,
    };
    u32::from(domain::is_visible(state))
}

/// Advances a scalar lifecycle state or returns `-1` for an invalid pair.
///
/// States are pending `0`, active `1`, deleting `2`, deleted `3`. Events are
/// provisioned `0`, begin-deletion `1`, and tuples-removed `2`.
///
/// ```
/// use openfga_axum_browser_demo::next_state;
///
/// assert_eq!(next_state(0, 0), 1);
/// assert_eq!(next_state(1, 1), 2);
/// assert_eq!(next_state(2, 2), 3);
/// assert_eq!(next_state(3, 0), -1);
/// assert_eq!(next_state(1, u32::MAX), -1);
/// ```
// SAFETY: this cdylib owns this export name, with a scalar-only C ABI.
#[unsafe(no_mangle)]
pub extern "C" fn next_state(state: u32, event: u32) -> i32 {
    let state = match state {
        0 => domain::Lifecycle::Pending,
        1 => domain::Lifecycle::Active,
        2 => domain::Lifecycle::Deleting,
        3 => domain::Lifecycle::Deleted,
        _ => return -1,
    };
    let event = match event {
        0 => domain::Event::Provisioned,
        1 => domain::Event::BeginDeletion,
        2 => domain::Event::TuplesRemoved,
        _ => return -1,
    };
    match domain::transition(state, event) {
        Some(domain::Lifecycle::Pending) => 0,
        Some(domain::Lifecycle::Active) => 1,
        Some(domain::Lifecycle::Deleting) => 2,
        Some(domain::Lifecycle::Deleted) => 3,
        None => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_visible, next_state};

    #[test]
    fn scalar_visibility_is_fail_closed() {
        for state in 0..=3 {
            assert_eq!(is_visible(state), u32::from(state == 1));
        }
        for state in [4, 255, u32::MAX] {
            assert_eq!(is_visible(state), 0);
        }
    }

    #[test]
    fn scalar_transition_table_preserves_the_wire_contract() {
        let expected = [[1, -1, -1], [-1, 2, -1], [-1, -1, 3], [-1, -1, -1]];
        for (state, row) in expected.iter().enumerate() {
            for (event, expected) in row.iter().enumerate() {
                assert_eq!(next_state(state as u32, event as u32), *expected);
            }
        }
    }

    #[test]
    fn unknown_values_do_not_wrap_into_valid_states_or_events() {
        for invalid in [4, 255, u32::MAX] {
            assert_eq!(next_state(invalid, 0), -1);
            assert_eq!(next_state(0, invalid), -1);
        }
        assert_eq!(next_state(0, 3), -1);
    }
}
