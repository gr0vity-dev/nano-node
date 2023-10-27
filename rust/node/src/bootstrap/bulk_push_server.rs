use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use super::BootstrapInitiator;
use crate::{
    block_processing::BlockProcessor,
    stats::{DetailType, Direction, StatType, Stats},
    transport::{SocketExtensions, TcpServer, TcpServerExt},
    utils::{AsyncRuntime, ErrorCode, ThreadPool},
};
use num_traits::FromPrimitive;
use rsnano_core::{
    deserialize_block_enum_with_type,
    utils::{Logger, StreamAdapter},
    work::WorkThresholds,
    BlockType, ChangeBlock, OpenBlock, ReceiveBlock, SendBlock, StateBlock,
};
use rsnano_ledger::Ledger;
use tokio::task::spawn_blocking;

/// Server side of a bulk_push request. Receives blocks and puts them in the block processor to be processed.
pub struct BulkPushServer {
    server_impl: Arc<Mutex<BulkPushServerImpl>>,
}

impl BulkPushServer {
    pub fn new(
        async_rt: Arc<AsyncRuntime>,
        connection: Arc<TcpServer>,
        ledger: Arc<Ledger>,
        logger: Arc<dyn Logger>,
        thread_pool: Arc<dyn ThreadPool>,
        enable_logging: bool,
        enable_network_logging: bool,
        block_processor: Arc<BlockProcessor>,
        bootstrap_initiator: Arc<BootstrapInitiator>,
        stats: Arc<Stats>,
        work_thresholds: WorkThresholds,
    ) -> Self {
        let server_impl = BulkPushServerImpl {
            async_rt,
            connection,
            enable_logging,
            enable_network_logging,
            ledger,
            logger,
            thread_pool: Arc::downgrade(&thread_pool),
            block_processor: Arc::downgrade(&block_processor),
            receive_buffer: Arc::new(Mutex::new(vec![0; 256])),
            bootstrap_initiator: Arc::downgrade(&bootstrap_initiator),
            stats,
            work_thresholds,
        };

        Self {
            server_impl: Arc::new(Mutex::new(server_impl)),
        }
    }

    pub fn throttled_receive(&self) {
        let server_impl2 = Arc::clone(&self.server_impl);
        self.server_impl
            .lock()
            .unwrap()
            .throttled_receive(server_impl2);
    }
}

struct BulkPushServerImpl {
    async_rt: Arc<AsyncRuntime>,
    ledger: Arc<Ledger>,
    logger: Arc<dyn Logger>,
    enable_logging: bool,
    enable_network_logging: bool,
    connection: Arc<TcpServer>,
    thread_pool: Weak<dyn ThreadPool>,
    block_processor: Weak<BlockProcessor>,
    receive_buffer: Arc<Mutex<Vec<u8>>>,
    bootstrap_initiator: Weak<BootstrapInitiator>,
    stats: Arc<Stats>,
    work_thresholds: WorkThresholds,
}

impl BulkPushServerImpl {
    fn throttled_receive(&self, server_impl: Arc<Mutex<Self>>) {
        let Some(thread_pool) = self.thread_pool.upgrade() else {
            return;
        };
        let Some(block_processor) = self.block_processor.upgrade() else {
            return;
        };
        if !block_processor.half_full() {
            self.receive(server_impl);
        } else {
            thread_pool.add_delayed_task(
                Duration::from_secs(1),
                Box::new(move || {
                    let server_impl2 = Arc::clone(&server_impl);
                    let guard = server_impl.lock().unwrap();
                    if !guard.connection.is_stopped() {
                        guard.throttled_receive(server_impl2);
                    }
                }),
            );
        }
    }

    fn receive(&self, server_impl: Arc<Mutex<Self>>) {
        let Some(bootstrap_initiator) = self.bootstrap_initiator.upgrade() else {
            return;
        };
        if bootstrap_initiator.in_progress() {
            if self.enable_logging {
                self.logger
                    .try_log("Aborting bulk_push because a bootstrap attempt is in progress");
            }
        } else {
            let socket = Arc::clone(&self.connection.socket);
            let buffer = Arc::clone(&self.receive_buffer);
            let server_impl2 = Arc::clone(&server_impl);
            self.async_rt.tokio.spawn(async move {
                let result = socket.read_raw(buffer, 1).await;
                spawn_blocking(Box::new(move || {
                    let guard = server_impl.lock().unwrap();
                    match result {
                        Ok(()) => {
                            guard.received_type(server_impl2);
                        }
                        Err(e) => {
                            if guard.enable_logging {
                                guard
                                    .logger
                                    .try_log(&format!("Error receiving block type: {:?}", e));
                            }
                        }
                    }
                }));
            });
        }
    }

    fn received_type(&self, server_impl: Arc<Mutex<Self>>) {
        let stats = Arc::clone(&self.stats);
        let server_impl2 = Arc::clone(&server_impl);
        let block_type = { BlockType::from_u8(self.receive_buffer.lock().unwrap()[0]) };
        let socket = Arc::clone(&self.connection.socket);
        let buffer = Arc::clone(&self.receive_buffer);

        match block_type {
            Some(BlockType::NotABlock) => {
                self.connection.start();
                return;
            }
            Some(BlockType::Invalid) | None => {
                if self.enable_network_logging {
                    self.logger.try_log("Unknown type received as block type");
                }
                return;
            }
            _ => {}
        }

        self.async_rt.tokio.spawn(async move {
            match block_type {
                Some(BlockType::LegacySend) => {
                    stats.inc(StatType::Bootstrap, DetailType::Send, Direction::In);
                    let result = socket.read_raw(buffer, SendBlock::serialized_size()).await;
                    let ec;
                    let len;
                    match result {
                        Ok(()) => {
                            ec = ErrorCode::new();
                            len = SendBlock::serialized_size();
                        }
                        Err(_) => {
                            ec = ErrorCode::fault();
                            len = 0;
                        }
                    }

                    spawn_blocking(Box::new(move || {
                        server_impl.lock().unwrap().received_block(
                            server_impl2,
                            ec,
                            len,
                            BlockType::LegacySend,
                        );
                    }));
                }
                Some(BlockType::LegacyReceive) => {
                    stats.inc(StatType::Bootstrap, DetailType::Receive, Direction::In);
                    let result = socket
                        .read_raw(buffer, ReceiveBlock::serialized_size())
                        .await;
                    let ec;
                    let len;
                    match result {
                        Ok(()) => {
                            ec = ErrorCode::new();
                            len = SendBlock::serialized_size();
                        }
                        Err(_) => {
                            ec = ErrorCode::fault();
                            len = 0;
                        }
                    }
                    spawn_blocking(Box::new(move || {
                        server_impl.lock().unwrap().received_block(
                            server_impl2,
                            ec,
                            len,
                            BlockType::LegacyReceive,
                        );
                    }));
                }
                Some(BlockType::LegacyOpen) => {
                    stats.inc(StatType::Bootstrap, DetailType::Open, Direction::In);
                    let result = socket.read_raw(buffer, OpenBlock::serialized_size()).await;
                    let ec;
                    let len;
                    match result {
                        Ok(()) => {
                            ec = ErrorCode::new();
                            len = SendBlock::serialized_size();
                        }
                        Err(_) => {
                            ec = ErrorCode::fault();
                            len = 0;
                        }
                    }
                    spawn_blocking(Box::new(move || {
                        server_impl.lock().unwrap().received_block(
                            server_impl2,
                            ec,
                            len,
                            BlockType::LegacyOpen,
                        );
                    }));
                }
                Some(BlockType::LegacyChange) => {
                    stats.inc(StatType::Bootstrap, DetailType::Change, Direction::In);
                    let result = socket
                        .read_raw(buffer, ChangeBlock::serialized_size())
                        .await;
                    let ec;
                    let len;
                    match result {
                        Ok(()) => {
                            ec = ErrorCode::new();
                            len = SendBlock::serialized_size();
                        }
                        Err(_) => {
                            ec = ErrorCode::fault();
                            len = 0;
                        }
                    }
                    spawn_blocking(Box::new(move || {
                        server_impl.lock().unwrap().received_block(
                            server_impl2,
                            ec,
                            len,
                            BlockType::LegacyChange,
                        );
                    }));
                }
                Some(BlockType::State) => {
                    stats.inc(StatType::Bootstrap, DetailType::StateBlock, Direction::In);
                    let result = socket.read_raw(buffer, StateBlock::serialized_size()).await;
                    let ec;
                    let len;
                    match result {
                        Ok(()) => {
                            ec = ErrorCode::new();
                            len = SendBlock::serialized_size();
                        }
                        Err(_) => {
                            ec = ErrorCode::fault();
                            len = 0;
                        }
                    }
                    spawn_blocking(Box::new(move || {
                        server_impl.lock().unwrap().received_block(
                            server_impl2,
                            ec,
                            len,
                            BlockType::State,
                        );
                    }));
                }
                Some(BlockType::NotABlock) | Some(BlockType::Invalid) | None => unreachable!(),
            }
        });
    }

    fn received_block(
        &self,
        server_impl: Arc<Mutex<Self>>,
        ec: ErrorCode,
        _len: usize,
        block_type: BlockType,
    ) {
        let Some(block_processor) = self.block_processor.upgrade() else {
            return;
        };

        if ec.is_ok() {
            let guard = self.receive_buffer.lock().unwrap();
            let block =
                deserialize_block_enum_with_type(block_type, &mut StreamAdapter::new(&guard));
            drop(guard);
            match block {
                Ok(block) => {
                    if self.work_thresholds.validate_entry_block(&block) {
                        if self.enable_logging {
                            self.logger.try_log(&format!(
                                "Insufficient work for bulk push block: {}",
                                block.hash()
                            ));
                        }
                        self.stats.inc_detail_only(
                            StatType::Error,
                            DetailType::InsufficientWork,
                            Direction::In,
                        );
                    } else {
                        block_processor.process_active(Arc::new(block));
                        self.throttled_receive(server_impl);
                    }
                }
                Err(_) => {
                    if self.enable_logging {
                        self.logger
                            .try_log("Error deserializing block received from pull request");
                    }
                }
            }
        }
    }
}
