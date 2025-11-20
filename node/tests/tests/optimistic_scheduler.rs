use std::time::Duration;

use rsnano_node::consensus::election::ElectionBehavior;
use rsnano_types::DEV_GENESIS_KEY;
use test_helpers::{System, assert_never, assert_timely, assert_timely2, setup_chains};

/*
 * Ensure account gets activated for a single unconfirmed account chain
 */
#[test]
pub fn activate_one() {
    let mut system = System::new();
    let node = system.make_node();
    let consensus_services = node.consensus_subsystem().test_handles();

    // Needs to be greater than optimistic scheduler `gap_threshold`
    let howmany_blocks = 64;

    let chains = setup_chains(
        &node,
        /* single chain */ 1,
        howmany_blocks,
        &DEV_GENESIS_KEY,
        /* do not confirm */ false,
    );
    let (_, blocks) = chains.first().unwrap();

    // Confirm block towards at the beginning the chain, so gap between confirmation
    // and account frontier is larger than `gap_threshold`
    node.confirm(blocks[11].hash());

    // Ensure unconfirmed account head block gets activated
    let block = blocks.last().unwrap();
    assert_timely2(|| node.is_active_root(&block.qualified_root()));

    assert_eq!(
        consensus_services
            .active
            .read()
            .unwrap()
            .election_for_root(&block.qualified_root())
            .unwrap()
            .behavior(),
        ElectionBehavior::Optimistic
    );
}

/*
 * Ensure account gets activated for a single unconfirmed account chain with nothing yet confirmed
 */
#[test]
pub fn activate_one_zero_conf() {
    let mut system = System::new();
    let node = system.make_node();
    let consensus_services = node.consensus_subsystem().test_handles();

    // Can be smaller than optimistic scheduler `gap_threshold`
    // This is meant to activate short account chains (eg. binary tree spam leaf accounts)
    let howmany_blocks = 6;

    let chains = setup_chains(
        &node,
        /* single chain */ 1,
        howmany_blocks,
        &DEV_GENESIS_KEY,
        /* do not confirm */ false,
    );
    let (_, blocks) = chains.first().unwrap();

    // Ensure unconfirmed account head block gets activated
    let block = blocks.last().unwrap();

    assert_timely2(|| node.is_active_root(&block.qualified_root()));

    assert_eq!(
        consensus_services
            .active
            .read()
            .unwrap()
            .election_for_root(&block.qualified_root())
            .unwrap()
            .behavior(),
        ElectionBehavior::Optimistic
    );
}

/*
 * Ensure account gets activated for a multiple unconfirmed account chains
 */
#[test]
pub fn activate_many() {
    let mut system = System::new();
    let node = system.make_node();
    let consensus_services = node.consensus_subsystem().test_handles();

    // Needs to be greater than optimistic scheduler `gap_threshold`
    let howmany_blocks = 64;
    let howmany_chains = 16;

    let chains = setup_chains(
        &node,
        howmany_chains,
        howmany_blocks,
        &DEV_GENESIS_KEY,
        /* do not confirm */ false,
    );

    // Ensure all unconfirmed accounts head block gets activated
    assert_timely(Duration::from_secs(20), || {
        chains.iter().all(|(_, blocks)| {
            let block = blocks.last().unwrap();
            let active = consensus_services.active.read().unwrap();
            let Some(election) = active.election_for_root(&block.qualified_root()) else {
                return false;
            };

            let behavior = election.behavior();
            behavior == ElectionBehavior::Optimistic
        })
    });
}

/*
 * Ensure accounts with some blocks already confirmed and with less than `gap_threshold` blocks do not get activated
 */
#[test]
pub fn under_gap_threshold() {
    let mut system = System::new();
    let node = system
        .build_node()
        .config(System::default_config_without_backlog_scan())
        .finish();

    // Must be smaller than optimistic scheduler `gap_threshold`
    let howmany_blocks = 64;

    let chains = setup_chains(
        &node,
        1,
        howmany_blocks,
        &DEV_GENESIS_KEY,
        /* do not confirm */ false,
    );

    let (_, blocks) = chains.first().unwrap();

    // Confirm block towards the end of the chain, so gap between confirmation and account frontier is less than `gap_threshold`
    node.confirm(blocks[55].hash());

    // Manually trigger backlog scan
    node.backlog_scan.trigger();

    // Ensure unconfirmed account head block gets activated
    let block = blocks.last().unwrap();
    assert_never(Duration::from_secs(3), || {
        node.is_active_hash(&block.hash())
    });
}
