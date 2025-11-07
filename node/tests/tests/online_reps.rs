use rsnano_types::{Amount, DEV_GENESIS_KEY};
use std::time::Duration;
use test_helpers::{System, assert_always_eq, assert_timely_eq2};

// Online reps should be able to observe remote representative
#[test]
fn observe() {
    let mut system = System::new();
    let node = system.make_node();
    assert_eq!(
        Amount::ZERO,
        node.services().online_reps.lock().unwrap().online_weight()
    );

    // Add genesis representative
    let node_rep = system.make_node();
    node_rep.insert_into_wallet(&DEV_GENESIS_KEY);

    // The node should see that weight as online
    assert_timely_eq2(
        || node.services().online_reps.lock().unwrap().online_weight(),
        Amount::MAX,
    );
}

// Online weight calculation should include local representative
#[test]
fn observe_local() {
    let mut system = System::new();
    let node = system.make_node();
    node.insert_into_wallet(&DEV_GENESIS_KEY);
    assert_timely_eq2(
        || node.services().online_reps.lock().unwrap().online_weight(),
        Amount::MAX,
    );
    assert_always_eq(
        Duration::from_secs(1),
        || node.services().online_reps.lock().unwrap().online_weight(),
        Amount::MAX,
    );
}
