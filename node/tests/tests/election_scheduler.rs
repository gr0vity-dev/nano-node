use test_helpers::{System, assert_timely_eq2, assert_timely2};

mod election_scheduler {
    use std::time::Duration;

    use super::*;
    use rsnano_ledger::{DEV_GENESIS_ACCOUNT, test_helpers::UnsavedBlockLatticeBuilder};
    use rsnano_node::{
        config::NodeConfig,
        consensus::{
            AecInsertRequest, election::ElectionBehavior,
            election_schedulers::OptimisticSchedulerConfig,
        },
    };
    use rsnano_types::{Amount, BlockPriority, DEV_GENESIS_KEY, PrivateKey};
    use test_helpers::{setup_chains, setup_rep};

    #[test]
    fn activate_one_timely() {
        let mut system = System::new();
        let node = system.make_node();

        let mut lattice = UnsavedBlockLatticeBuilder::new();
        let send1 = lattice
            .genesis()
            .send(&*DEV_GENESIS_KEY, Amount::nano(1000));

        let ledger_services = node.ledger_query_services();
        let consensus_services = node.consensus_subsystem().test_handles();
        ledger_services.ledger_arc().process_one(&send1).unwrap();

        consensus_services
            .election_schedulers
            .priority
            .activate(&ledger_services.ledger_arc().any(), &*DEV_GENESIS_ACCOUNT);

        assert_timely2(|| node.is_active_root(&send1.qualified_root()));
    }

    #[test]
    fn activate_one_flush() {
        let mut system = System::new();
        let node = system.make_node();
        let mut lattice = UnsavedBlockLatticeBuilder::new();

        // Create a send block
        let send1 = lattice
            .genesis()
            .send(&*DEV_GENESIS_KEY, Amount::nano(1000));

        // Process the block
        let ledger_services = node.ledger_query_services();
        let consensus_services = node.consensus_subsystem().test_handles();
        ledger_services.ledger_arc().process_one(&send1).unwrap();

        // Activate the account
        consensus_services
            .election_schedulers
            .priority
            .activate(&ledger_services.ledger_arc().any(), &*DEV_GENESIS_ACCOUNT);

        // Assert that the election is created within 5 seconds
        assert_timely2(|| node.is_active_root(&send1.qualified_root()));
    }

    #[test]
    /**
     * Tests that the election scheduler and the active transactions container (AEC)
     * work in sync with regards to the node configuration value "active_elections_size".
     *
     * The test sets up two forcefully cemented blocks -- a send on the genesis account and a receive on a second account.
     * It then creates two other blocks, each a successor to one of the previous two,
     * and processes them locally (without the node starting elections for them, but just saving them to disk).
     *
     * Elections for these latter two (B1 and B2) are started by the test code manually via `election_scheduler::activate`.
     * The test expects E1 to start right off and take its seat into the AEC.
     * E2 is expected not to start though (because the AEC is full), so B2 should be awaiting in the scheduler's queue.
     *
     * As soon as the test code manually confirms E1 (and thus evicts it out of the AEC),
     * it is expected that E2 begins and the scheduler's queue becomes empty again.
     */
    fn no_vacancy() {
        let mut system = System::new();
        let node = system
            .build_node()
            .config(NodeConfig {
                active_elections: rsnano_node::consensus::ActiveElectionsConfig {
                    max_elections: 1,
                    ..Default::default()
                },
                ..System::default_config_without_backlog_scan()
            })
            .finish();

        let mut lattice = UnsavedBlockLatticeBuilder::new();
        let key = PrivateKey::new();

        // Activating accounts depends on confirmed dependencies. First, prepare 2 accounts
        let send = lattice.genesis().send(&key, Amount::nano(1000));
        let send = node.process(send.clone());
        let consensus_services = node.consensus_subsystem().test_handles();
        consensus_services.confirming_set.add_block(send.hash());

        let receive = lattice.account(&key).receive(&send);
        let receive = node.process(receive.clone());
        consensus_services.confirming_set.add_block(receive.hash());

        assert_timely2(|| {
            node.block_confirmed(&send.hash()) && node.block_confirmed(&receive.hash())
        });

        // Second, process two eligible transactions
        let block1 = lattice
            .genesis()
            .send(&*DEV_GENESIS_KEY, Amount::nano(1000));
        node.process(block1.clone());

        // There is vacancy so it should be inserted
        let ledger_services = node.ledger_query_services();
        consensus_services
            .election_schedulers
            .priority
            .activate(&ledger_services.ledger_arc().any(), &DEV_GENESIS_ACCOUNT);
        assert_timely2(|| node.is_active_root(&block1.qualified_root()));

        let block2 = lattice.account(&key).send(&key, Amount::nano(1000));
        node.process(block2.clone());

        // There is no vacancy so it should stay queued
        consensus_services
            .election_schedulers
            .priority
            .activate(&ledger_services.ledger_arc().any(), &key.account());
        let election_schedulers = consensus_services.election_schedulers.clone();
        let election_schedulers_for_len = election_schedulers.clone();
        assert_timely_eq2(|| election_schedulers_for_len.priority.len(), 1);
        assert_eq!(node.is_active_root(&block2.qualified_root()), false);

        // Election confirmed, next in queue should begin
        node.force_confirm(&block1.hash());
        assert_timely2(|| node.is_active_root(&block2.qualified_root()));
        assert!(election_schedulers.priority.is_empty());
    }

    /*
     * Tests that an optimistic election can be transitioned to a priority election.
     *
     * The test:
     * 1. Creates a chain of 2 blocks with an optimistic election for the second block
     * 2. Confirms the first block in the chain
     * 3. Attempts to start a priority election for the second block
     * 4. Verifies that the existing optimistic election is transitioned to priority
     */
    #[test]
    fn transition_optimistic_to_priority() {
        let mut system = System::new();
        system.network_params.network.vote_broadcast_interval = Duration::from_secs(15);

        let node = system
            .build_node()
            .config(NodeConfig {
                optimistic_scheduler: OptimisticSchedulerConfig {
                    gap_threshold: 1,
                    ..Default::default()
                },
                enable_voting: true,
                enable_hinted_scheduler: false,
                ..System::default_config()
            })
            .finish();

        // Add representative
        let rep_weight = Amount::nano(100_000);
        let rep = setup_rep(&node, rep_weight, &DEV_GENESIS_KEY);
        node.wallet_services().insert_into_wallet(&rep);

        // Create a chain of blocks - and trigger an optimistic election for the last block
        let howmany_blocks = 2;
        let chains = setup_chains(
            &node,
            /* single chain */ 1,
            howmany_blocks,
            &DEV_GENESIS_KEY,
            /* do not confirm */ false,
        );
        let (_account, blocks) = &chains[0];

        // Wait for optimistic election to start for last block
        let block = blocks.last().unwrap();
        assert_timely2(|| node.is_active_hash(&block.hash()));
        let consensus_services = node.consensus_subsystem().test_handles();
        assert_eq!(
            consensus_services
                .active
                .read()
                .unwrap()
                .election_for_block(&block.hash())
                .unwrap()
                .behavior(),
            ElectionBehavior::Optimistic
        );

        // Confirm first block to allow upgrading second block's election
        node.confirm(blocks[howmany_blocks - 1].hash());

        // Attempt to start priority election for second block
        let network_services = node.network_subsystem().test_handles();
        let _ = consensus_services.active.write().unwrap().insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::MIN),
            network_services.steady_clock.now(),
        );

        // Verify priority transition
        assert_eq!(
            consensus_services
                .active
                .read()
                .unwrap()
                .election_for_block(&block.hash())
                .unwrap()
                .behavior(),
            ElectionBehavior::Priority
        );
        assert!(node.is_active_root(&block.qualified_root()));
    }
}
