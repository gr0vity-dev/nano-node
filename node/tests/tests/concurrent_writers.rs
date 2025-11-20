use rsnano_ledger::WriterType;
use rsnano_network::ChannelId;
use rsnano_node::{
    block_processing::{BlockContext, BlockSource},
    config::NodeConfig,
};
use rsnano_types::{Amount, PrivateKey};
use test_helpers::{System, assert_timely};

#[test]
fn block_processor_and_confirming_set_make_progress_concurrently() {
    let mut config = System::default_config_without_backlog_scan();
    config.block_processor_threads = 2;
    config.block_processor.batch_size = 1;
    config.confirming_set.batch_size = 1;

    let mut system = System::new();
    let node = system.build_node().config(config).finish();

    // Create two independent chains to drive concurrent processing/confirmation.
    let key1 = PrivateKey::from(41);
    let key2 = PrivateKey::from(42);

    let send1 = node
        .ledger_query_services()
        .ledger
        .any()
        .genesis_send(&key1, Amount::nano(1));
    let send2 = node
        .ledger_query_services()
        .ledger
        .any()
        .genesis_send(&key2, Amount::nano(1));

    // Push blocks through the block processor.
    for block in [&send1, &send2] {
        let ctx = BlockContext::new(block.clone(), BlockSource::Local, ChannelId::LOOPBACK);
        assert!(node
            .consensus_subsystem().test_handles()
            .block_processor_queue
            .push(ctx));
    }

    assert_timely(std::time::Duration::from_secs(5), || {
        node.ledger_query_services()
            .ledger
            .any()
            .block_exists(&send1.hash())
            && node
                .ledger_query_services()
                .ledger
                .any()
                .block_exists(&send2.hash())
    });

    // Drive confirmation height processing concurrently.
    node.consensus_subsystem().test_handles()
        .confirming_set
        .add_block(send1.hash());
    node.consensus_subsystem().test_handles()
        .confirming_set
        .add_block(send2.hash());

    assert_timely(std::time::Duration::from_secs(5), || {
        node.ledger_query_services()
            .ledger
            .confirmed()
            .block_exists(&send1.hash())
            && node
                .ledger_query_services()
                .ledger
                .confirmed()
                .block_exists(&send2.hash())
    });

    let bp_stats = node.consensus_subsystem().test_handles().block_processor.stats();
    assert!(
        bp_stats.max_optimistic_concurrency() >= 2,
        "expected block processor optimistic concurrency to reach at least 2"
    );

    let conf_stats = node.consensus_subsystem().test_handles().confirming_set.writer_stats();
    assert!(
        conf_stats.max_optimistic_concurrency() >= 1,
        "confirmation height writer should have executed optimistically"
    );
}
