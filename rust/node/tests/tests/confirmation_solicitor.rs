use std::sync::Arc;

use rsnano_core::{Account, Amount, BlockEnum, StateBlock, DEV_GENESIS_KEY};
use rsnano_ledger::{DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH};
use rsnano_messages::ConfirmReq;
use rsnano_node::{
    config::NodeFlags,
    consensus::{ConfirmationSolicitor, Election, ElectionBehavior},
    representatives::PeeredRep,
    stats::{DetailType, Direction, StatType},
    DEV_NETWORK_PARAMS,
};

use super::helpers::{establish_tcp, System};

#[test]
fn batches() {
    let mut system = System::new();
    let mut flags = NodeFlags::default();
    flags.disable_request_loop = true;
    flags.disable_rep_crawler = true;
    let node1 = system.build_node().flags(flags.clone()).finish();
    let node2 = system.build_node().flags(flags).finish();
    let channel1 = establish_tcp(&node2, &node1);
    // Solicitor will only solicit from this representative
    let representative = PeeredRep::new(
        *DEV_GENESIS_ACCOUNT,
        channel1.channel_id(),
        node2.relative_time.elapsed(),
    );
    let representatives = vec![representative];

    let mut solicitor = ConfirmationSolicitor::new(&DEV_NETWORK_PARAMS, &node2.network);
    solicitor.prepare(&representatives);

    let send = Arc::new(BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        *DEV_GENESIS_HASH,
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::raw(100),
        Account::from(123).into(),
        &DEV_GENESIS_KEY,
        node1.work_generate_dev((*DEV_GENESIS_HASH).into()),
    )));

    {
        for i in 0..ConfirmReq::HASHES_MAX {
            let election = Election::new(
                i,
                send.clone(),
                ElectionBehavior::Priority,
                Box::new(|_| {}),
                Box::new(|_| {}),
            );

            let data = election.mutex.lock().unwrap();
            assert_eq!(solicitor.add(&election, &data), false);
        }
        // Reached the maximum amount of requests for the channel
        let election = Election::new(
            1000,
            send.clone(),
            ElectionBehavior::Priority,
            Box::new(|_| {}),
            Box::new(|_| {}),
        );
        // Broadcasting should be immediate
        assert_eq!(
            0,
            node2
                .stats
                .count(StatType::Message, DetailType::Publish, Direction::Out)
        );
        let data = election.mutex.lock().unwrap();
        solicitor.broadcast(&data).unwrap();
    }
    // One publish through directed broadcasting and another through random flooding
    assert_eq!(
        2,
        node2
            .stats
            .count(StatType::Message, DetailType::Publish, Direction::Out)
    );
    solicitor.flush();
    assert_eq!(
        1,
        node2
            .stats
            .count(StatType::Message, DetailType::ConfirmReq, Direction::Out)
    );
}
