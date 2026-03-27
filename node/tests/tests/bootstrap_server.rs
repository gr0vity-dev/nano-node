use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    thread,
    time::Duration,
};

use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_messages::{
    AccountInfoReqPayload, AscPullAck, AscPullAckType, AscPullReq, AscPullReqType,
    BlocksReqPayload, FrontiersReqPayload, HashType, Message,
};
use rsnano_node::{
    Node, NodeBuilder, NodeCallbacks, bootstrap::BootstrapServer, config::NetworkParams,
    unique_path,
};
use rsnano_types::{
    Account, Block, BlockHash, DEV_GENESIS_KEY, HashOrAccount, NetworkType, SavedBlock, WalletId,
};
use rsnano_utils::stats::{DetailType, Direction, StatType};
use test_helpers::{
    System, assert_always_eq, assert_timely_eq, assert_timely_eq2, make_fake_channel, setup_chains,
};

#[test]
fn serve_account_blocks() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let mut chains = setup_chains(&node, 1, 128, &DEV_GENESIS_KEY, true);
    let (first_account, first_blocks) = chains.pop().unwrap();

    // Request blocks from account root
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Account,
            start: first_account.into(),
            count: BootstrapServer::MAX_BLOCKS as u8,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Blocks(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.blocks().len(), 128);
    assert!(compare_blocks(response_payload.blocks(), &first_blocks));

    // Ensure we don't get any unexpected responses
    assert_always_eq(Duration::from_secs(1), || responses.len(), 1);
}

#[test]
fn serve_hash() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let mut chains = setup_chains(&node, 1, 256, &DEV_GENESIS_KEY, true);
    let (_, blocks) = chains.pop().unwrap();

    // Skip a few blocks to request hash in the middle of the chain
    let blocks = &blocks[9..];

    // Request blocks from the middle of the chain
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Block,
            start: blocks[0].hash().into(),
            count: BootstrapServer::MAX_BLOCKS as u8,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Blocks(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.blocks().len(), 128);
    assert!(compare_blocks(response_payload.blocks(), blocks));

    // Ensure we don't get any unexpected responses
    assert_always_eq(Duration::from_secs(1), || responses.len(), 1);
}

#[test]
fn serve_hash_one() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let mut chains = setup_chains(&node, 1, 256, &DEV_GENESIS_KEY, true);
    let (_account, blocks) = chains.pop().unwrap();

    // Skip a few blocks to request hash in the middle of the chain
    let blocks = &blocks[9..];

    // Request blocks from the middle of the chain
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Block,
            start: blocks[0].hash().into(),
            count: 1,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Blocks(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.blocks().len(), 1);
    assert_eq!(response_payload.blocks()[0].hash(), blocks[0].hash());
}

#[test]
fn serve_end_of_chain() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let mut chains = setup_chains(&node, 1, 128, &DEV_GENESIS_KEY, true);
    let (_account, blocks) = chains.pop().unwrap();

    // Request blocks from account frontier
    //
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Block,
            start: blocks.last().unwrap().hash().into(),
            count: BootstrapServer::MAX_BLOCKS as u8,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq(Duration::from_secs(5), || responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Blocks(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.blocks().len(), 1);
    assert_eq!(
        response_payload.blocks()[0].hash(),
        blocks.last().unwrap().hash()
    );
}

#[test]
fn serve_missing() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    setup_chains(&node, 1, 128, &DEV_GENESIS_KEY, true);

    // Request blocks from account frontier
    //
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Block,
            start: HashOrAccount::from(42),
            count: BootstrapServer::MAX_BLOCKS as u8,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Blocks(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.blocks().len(), 0);
}

#[test]
fn serve_multiple() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let chains = setup_chains(&node, 32, 16, &DEV_GENESIS_KEY, true);

    {
        // Request blocks from multiple chains at once
        let mut next_id = 0;
        for (account, _) in &chains {
            // Request blocks from account root
            let request = Message::AscPullReq(AscPullReq {
                id: next_id,
                req_type: AscPullReqType::Blocks(BlocksReqPayload {
                    start_type: HashType::Account,
                    start: (*account).into(),
                    count: BootstrapServer::MAX_BLOCKS as u8,
                }),
            });
            next_id += 1;

            let channel = make_fake_channel(&node);
            node.inbound_message_queue.put(request, channel);
        }
    }

    assert_timely_eq(Duration::from_secs(15), || responses.len(), chains.len());

    let all_responses = responses.get();
    {
        let mut next_id = 0;
        for (_, blocks) in &chains {
            // Find matching response
            let response = all_responses.iter().find(|r| r.id == next_id).unwrap();

            // Ensure we got response exactly for what we asked for

            let AscPullAckType::Blocks(ref response_payload) = response.pull_type else {
                panic!("wrong ack type")
            };

            assert_eq!(response_payload.blocks().len(), 17); // 1 open block + 16 random blocks
            assert!(compare_blocks(response_payload.blocks(), blocks));

            next_id += 1;
        }
    }
}

#[test]
fn serve_account_info() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let mut chains = setup_chains(&node, 1, 128, &DEV_GENESIS_KEY, true);
    let (account, blocks) = chains.pop().unwrap();

    // Request blocks from account root
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::AccountInfo(AccountInfoReqPayload {
            target: account.into(),
            target_type: HashType::Account,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::AccountInfo(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.account, account);
    assert_eq!(response_payload.account_open, blocks[0].hash());
    assert_eq!(response_payload.account_head, blocks.last().unwrap().hash());
    assert_eq!(response_payload.account_block_count as usize, blocks.len());
    assert_eq!(
        response_payload.account_conf_frontier,
        blocks.last().unwrap().hash()
    );
    assert_eq!(response_payload.account_conf_height as usize, blocks.len());

    // Ensure we don't get any unexpected responses
    assert_always_eq(Duration::from_secs(1), || responses.len(), 1);
}

#[test]
fn serve_account_info_missing() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    setup_chains(&node, 1, 128, &DEV_GENESIS_KEY, true);

    // Request blocks from account root
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::AccountInfo(AccountInfoReqPayload {
            target: HashOrAccount::from(42), // unknown account
            target_type: HashType::Account,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::AccountInfo(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.account, Account::from(42));
    assert_eq!(response_payload.account_open, BlockHash::ZERO);
    assert_eq!(response_payload.account_head, BlockHash::ZERO);
    assert_eq!(response_payload.account_block_count, 0);
    assert_eq!(response_payload.account_conf_frontier, BlockHash::ZERO);
    assert_eq!(response_payload.account_conf_height, 0);

    // Ensure we don't get any unexpected responses
    assert_always_eq(Duration::from_secs(1), || responses.len(), 1);
}

#[test]
fn serve_frontiers() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let chains = setup_chains(&node, 32, 4, &DEV_GENESIS_KEY, true);

    // Request all frontiers
    let request = Message::AscPullReq(AscPullReq {
        id: 7,
        req_type: AscPullReqType::Frontiers(FrontiersReqPayload {
            start: Account::ZERO,
            count: BootstrapServer::MAX_FRONTIERS as u16,
        }),
    });

    let channel = make_fake_channel(&node);
    node.inbound_message_queue.put(request, channel);

    assert_timely_eq2(|| responses.len(), 1);

    let response = responses.get().pop().unwrap();
    // Ensure we got response exactly for what we asked for
    assert_eq!(response.id, 7);
    let AscPullAckType::Frontiers(response_payload) = response.pull_type else {
        panic!("wrong ack type")
    };

    assert_eq!(response_payload.len(), chains.len() + 1); // +1 for genesis

    // Ensure frontiers match what we expect
    let mut expected_frontiers: HashMap<Account, BlockHash> = chains
        .iter()
        .map(|(account, blocks)| (*account, blocks.last().unwrap().hash()))
        .collect();
    expected_frontiers.insert(*DEV_GENESIS_ACCOUNT, node.latest(&DEV_GENESIS_ACCOUNT));

    for frontier in response_payload {
        assert_eq!(frontier.hash, expected_frontiers[&frontier.account]);
        expected_frontiers.remove(&frontier.account);
    }
    assert!(expected_frontiers.is_empty());
}

#[test]
fn serve_frontiers_invalid_count() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    setup_chains(&node, 4, 4, &DEV_GENESIS_KEY, true);

    // Zero count
    {
        let request = Message::AscPullReq(AscPullReq {
            id: 7,
            req_type: AscPullReqType::Frontiers(FrontiersReqPayload {
                start: Account::ZERO,
                count: 0,
            }),
        });

        let channel = make_fake_channel(&node);
        node.inbound_message_queue.put(request, channel);
    }

    assert_timely_eq(
        Duration::from_secs(5),
        || {
            node.stats.count(
                StatType::BootstrapServer,
                DetailType::Invalid,
                Direction::In,
            )
        },
        1,
    );

    // Count larger than allowed
    {
        let request = Message::AscPullReq(AscPullReq {
            id: 7,
            req_type: AscPullReqType::Frontiers(FrontiersReqPayload {
                start: Account::ZERO,
                count: BootstrapServer::MAX_FRONTIERS as u16 + 1,
            }),
        });

        let channel = make_fake_channel(&node);
        node.inbound_message_queue.put(request, channel);
    }

    assert_timely_eq(
        Duration::from_secs(5),
        || {
            node.stats.count(
                StatType::BootstrapServer,
                DetailType::Invalid,
                Direction::In,
            )
        },
        2,
    );

    // Max numeric value
    {
        let request = Message::AscPullReq(AscPullReq {
            id: 7,
            req_type: AscPullReqType::Frontiers(FrontiersReqPayload {
                start: Account::ZERO,
                count: u16::MAX,
            }),
        });

        let channel = make_fake_channel(&node);
        node.inbound_message_queue.put(request, channel);
    }

    assert_timely_eq(
        Duration::from_secs(5),
        || {
            node.stats.count(
                StatType::BootstrapServer,
                DetailType::Invalid,
                Direction::In,
            )
        },
        3,
    );
}

#[test]
fn bootstrap_server_shutdown_releases_owned_channels() {
    let mut system = System::new();
    let node = system.make_node();

    let responses = ResponseHelper::new();
    responses.connect(&node);

    let chains = setup_chains(&node, 32, 16, &DEV_GENESIS_KEY, true);
    let bootstrap_server = Arc::downgrade(&node.bootstrap_server);
    let channel_weaks = enqueue_block_requests(&node, &chains);

    assert_timely_eq(Duration::from_secs(15), || responses.len(), chains.len());

    system.stop_node(node);

    assert_timely_eq2(|| bootstrap_server.upgrade().is_none(), true);
    assert_timely_eq2(|| all_channels_dropped(&channel_weaks), true);
}

#[test]
fn bootstrap_server_stop_waits_for_response_callback_completion() {
    let mut system = System::new();
    let node = system.make_node();

    let callback = Arc::new(BlockingCallback::new());
    let callback_l = callback.clone();
    node.bootstrap_server
        .set_response_callback(Box::new(move |_response, _channel| {
            callback_l.wait_until_released();
        }));

    let chains = setup_chains(&node, 1, 16, &DEV_GENESIS_KEY, true);
    let bootstrap_server = Arc::downgrade(&node.bootstrap_server);
    let channel_weaks = enqueue_block_requests(&node, &chains);

    assert_timely_eq2(|| callback.entered(), 1);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let stopper = std::thread::spawn(move || {
        system.stop_node(node);
        tx.send(()).unwrap();
    });

    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).is_err(), true);

    callback.release();

    assert_timely_eq2(|| callback.exited(), 1);
    assert_timely_eq2(|| rx.recv_timeout(Duration::from_secs(5)).is_ok(), true);
    stopper.join().unwrap();

    assert_timely_eq2(|| bootstrap_server.upgrade().is_none(), true);
    assert_timely_eq2(|| all_channels_dropped(&channel_weaks), true);
}

#[test]
fn bootstrap_server_stop_waits_for_response_publish_completion() {
    let callback = Arc::new(BlockingCallback::new());
    let callback_l = callback.clone();
    let fixture = CallbackNode::new(
        NodeCallbacks::builder()
            .on_publish(move |_channel_id, message| {
                if matches!(message, Message::AscPullAck(_)) {
                    callback_l.wait_until_released();
                }
            })
            .finish(),
    );
    let node = &fixture.node;

    let chains = setup_chains(node, 1, 16, &DEV_GENESIS_KEY, true);
    let bootstrap_server = Arc::downgrade(&node.bootstrap_server);
    let channel_weaks = enqueue_block_requests(node, &chains);

    assert_timely_eq2(|| callback.entered(), 1);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let stopper = thread::spawn(move || {
        fixture.shutdown();
        tx.send(()).unwrap();
    });

    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).is_err(), true);

    callback.release();

    assert_timely_eq2(|| callback.exited(), 1);
    assert_timely_eq2(|| rx.recv_timeout(Duration::from_secs(5)).is_ok(), true);
    stopper.join().unwrap();

    assert_timely_eq2(|| bootstrap_server.upgrade().is_none(), true);
    assert_timely_eq2(|| all_channels_dropped(&channel_weaks), true);
}

#[test]
fn bootstrap_server_shutdown_does_not_wait_for_inbound_callback_completion() {
    let callback = Arc::new(BlockingCallback::new());
    let callback_l = callback.clone();
    let fixture = CallbackNode::new(
        NodeCallbacks::builder()
            .on_inbound(move |_channel_id, message| {
                if matches!(message, Message::AscPullReq(_)) {
                    callback_l.wait_until_released();
                }
            })
            .finish(),
    );
    let node = &fixture.node;

    let chains = setup_chains(node, 1, 16, &DEV_GENESIS_KEY, true);
    let bootstrap_server = Arc::downgrade(&node.bootstrap_server);
    let request = block_request(chains[0].0, 0);
    let channel = make_fake_channel(node);
    let channel_weak = Arc::downgrade(&channel);
    let inbound_queue = node.inbound_message_queue.clone();

    let enqueue = thread::spawn(move || {
        inbound_queue.put(request, channel);
    });

    assert_timely_eq2(|| callback.entered(), 1);

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let stopper = thread::spawn(move || {
        fixture.shutdown();
        tx.send(()).unwrap();
    });

    assert_timely_eq2(|| rx.recv_timeout(Duration::from_secs(5)).is_ok(), true);
    assert_eq!(callback.exited(), 0);

    callback.release();

    enqueue.join().unwrap();
    stopper.join().unwrap();

    assert_timely_eq2(|| callback.exited(), 1);
    assert_timely_eq2(|| bootstrap_server.upgrade().is_none(), true);
    assert_timely_eq2(|| channel_weak.upgrade().is_none(), true);
}

struct ResponseHelper {
    responses: Arc<Mutex<Vec<AscPullAck>>>,
}

impl ResponseHelper {
    fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn len(&self) -> usize {
        self.responses.lock().unwrap().len()
    }

    fn get(&self) -> Vec<AscPullAck> {
        self.responses.lock().unwrap().clone()
    }

    fn connect(&self, node: &Node) {
        let responses = self.responses.clone();
        node.bootstrap_server
            .set_response_callback(Box::new(move |response, _channel| {
                responses.lock().unwrap().push(response.clone());
            }));
    }
}

fn enqueue_block_requests(
    node: &Node,
    chains: &[(Account, Vec<SavedBlock>)],
) -> Vec<Weak<rsnano_network::Channel>> {
    let mut next_id = 0;
    let mut channels = Vec::new();

    for (account, _) in chains {
        let request = Message::AscPullReq(AscPullReq {
            id: next_id,
            req_type: AscPullReqType::Blocks(BlocksReqPayload {
                start_type: HashType::Account,
                start: (*account).into(),
                count: BootstrapServer::MAX_BLOCKS as u8,
            }),
        });
        next_id += 1;

        let channel = make_fake_channel(node);
        node.inbound_message_queue.put(request, channel.clone());
        channels.push(Arc::downgrade(&channel));
    }

    channels
}

fn block_request(account: Account, id: u64) -> Message {
    Message::AscPullReq(AscPullReq {
        id,
        req_type: AscPullReqType::Blocks(BlocksReqPayload {
            start_type: HashType::Account,
            start: account.into(),
            count: BootstrapServer::MAX_BLOCKS as u8,
        }),
    })
}

fn all_channels_dropped(channels: &[Weak<rsnano_network::Channel>]) -> bool {
    channels.iter().all(|channel| channel.upgrade().is_none())
}

struct CallbackNode {
    node: Arc<Node>,
    data_path: PathBuf,
}

impl CallbackNode {
    fn new(callbacks: NodeCallbacks) -> Self {
        let data_path = unique_path().expect("Could not get a unique path");
        let network = NetworkType::NanoDevNetwork;
        let mut node = NodeBuilder::new(network)
            .data_path(data_path.clone())
            .config(System::default_config())
            .network_params(NetworkParams::new(network))
            .callbacks(callbacks)
            .finish()
            .unwrap();
        node.wallets.create(WalletId::random());
        node.start();

        Self {
            node: Arc::new(node),
            data_path,
        }
    }

    fn shutdown(self) {
        shutdown_node(self.node, self.data_path);
    }
}

fn shutdown_node(mut node: Arc<Node>, data_path: PathBuf) {
    let start = std::time::Instant::now();
    loop {
        let n = Arc::get_mut(&mut node);
        if let Some(n) = n {
            n.stop();
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("Could not get exclusive access to node!");
        }
        std::thread::yield_now();
    }
    drop(node);
    std::fs::remove_dir_all(&data_path).expect("Could not delete node data dir");
}

struct BlockingCallback {
    state: Mutex<BlockingCallbackState>,
    condition: std::sync::Condvar,
}

#[derive(Default)]
struct BlockingCallbackState {
    entered: usize,
    exited: usize,
    released: bool,
}

impl BlockingCallback {
    fn new() -> Self {
        Self {
            state: Mutex::new(BlockingCallbackState::default()),
            condition: std::sync::Condvar::new(),
        }
    }

    fn wait_until_released(&self) {
        let mut state = self.state.lock().unwrap();
        state.entered += 1;
        self.condition.notify_all();

        while !state.released {
            state = self.condition.wait(state).unwrap();
        }

        state.exited += 1;
        self.condition.notify_all();
    }

    fn entered(&self) -> usize {
        self.state.lock().unwrap().entered
    }

    fn exited(&self) -> usize {
        self.state.lock().unwrap().exited
    }

    fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.released = true;
        self.condition.notify_all();
    }
}

/// Checks if both lists contain the same blocks, with `blocks_b`
fn compare_blocks(blocks_a: &VecDeque<Block>, blocks_b: &[SavedBlock]) -> bool {
    blocks_a
        .iter()
        .zip(blocks_b.iter().map(|b| &**b))
        .all(|(a, b)| a == b)
}
