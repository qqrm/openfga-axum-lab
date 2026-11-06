// Browser demo reuses the production domain functions; it does not run Axum.
#[path = "../src/domain.rs"]
#[allow(dead_code)]
mod domain;

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
