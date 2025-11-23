use super::WebsocketListener;
use rsnano_messages::TelemetryData;
use rsnano_node::{
    CompositeNodeEventHandler, NodeEvent, NodeEventHandler, TelemetryServices, WalletServices,
    config::WebsocketConfig, handles::LedgerQueryHandle,
};
use rsnano_types::{Account, BlockHash, Vote, VoteError};
use rsnano_websocket_messages::{MessageEnvelope, Topic, new_block_arrived_message};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr, SocketAddrV6},
    sync::Arc,
    time::UNIX_EPOCH,
};
use tokio::runtime::Handle;
use tracing::error;

pub fn create_websocket_server(
    config: WebsocketConfig,
    wallet_services: WalletServices,
    ledger: LedgerQueryHandle,
    telemetry_services: TelemetryServices,
    runtime: Handle,
    event_handlers: &mut CompositeNodeEventHandler,
) -> Option<Arc<WebsocketListener>> {
    if !config.enabled {
        return None;
    }

    let Ok(address) = config.address.parse::<IpAddr>() else {
        error!(address = config.address, "invalid websocket IP address");
        return None;
    };

    let endpoint = SocketAddr::new(address, config.port);
    let server = Arc::new(WebsocketListener::new(
        endpoint,
        wallet_services.clone(),
        ledger.clone(),
        runtime,
    ));

    event_handlers.add(NodeEventProcessor {
        server: server.clone(),
        ledger: ledger.clone(),
    });

    let server_w = Arc::downgrade(&server);
    telemetry_services.on_telemetry_processed(Box::new(move |data, peer_addr| {
        if let Some(server) = server_w.upgrade()
            && server.any_subscriber(Topic::Telemetry)
        {
            server.broadcast(&telemetry_received(data, *peer_addr));
        }
    }));

    Some(server)
}

fn telemetry_received(data: &TelemetryData, endpoint: SocketAddrV6) -> MessageEnvelope {
    MessageEnvelope::new(
        Topic::Telemetry,
        TelemetryReceived {
            block_count: data.block_count.to_string(),
            cemented_count: data.cemented_count.to_string(),
            unchecked_count: data.unchecked_count.to_string(),
            account_count: data.account_count.to_string(),
            bandwidth_cap: data.bandwidth_cap.to_string(),
            peer_count: data.peer_count.to_string(),
            protocol_version: data.protocol_version.to_string(),
            uptime: data.uptime.to_string(),
            genesis_block: data.genesis_block.to_string(),
            major_version: data.major_version.to_string(),
            minor_version: data.minor_version.to_string(),
            patch_version: data.patch_version.to_string(),
            pre_release_version: data.pre_release_version.to_string(),
            maker: data.maker.to_string(),
            timestamp: data
                .timestamp
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
            active_difficulty: format!("{:016x}", data.active_difficulty),
            node_id: data.node_id.to_string(),
            signature: data.signature.encode_hex(),
            address: endpoint.ip().to_string(),
            port: endpoint.port().to_string(),
        },
    )
}

#[derive(Serialize, Deserialize)]
pub struct TelemetryReceived {
    pub block_count: String,
    pub cemented_count: String,
    pub unchecked_count: String,
    pub account_count: String,
    pub bandwidth_cap: String,
    pub peer_count: String,
    pub protocol_version: String,
    pub uptime: String,
    pub genesis_block: String,
    pub major_version: String,
    pub minor_version: String,
    pub patch_version: String,
    pub pre_release_version: String,
    pub maker: String,
    pub timestamp: String,
    pub active_difficulty: String,
    pub node_id: String,
    pub signature: String,
    pub address: String,
    pub port: String,
}

fn started_election(hash: &BlockHash) -> MessageEnvelope {
    MessageEnvelope::new(
        Topic::StartedElection,
        StartedElection {
            hash: hash.to_string(),
        },
    )
}

#[derive(Serialize)]
struct StartedElection {
    hash: String,
}

fn stopped_election(hash: &BlockHash) -> MessageEnvelope {
    MessageEnvelope::new(
        Topic::StoppedElection,
        StoppedElection {
            hash: hash.to_string(),
        },
    )
}

#[derive(Serialize)]
struct StoppedElection {
    hash: String,
}

pub fn vote_received(vote: &Vote, result: Result<(), VoteError>) -> MessageEnvelope {
    MessageEnvelope::new(
        Topic::Vote,
        VoteReceived {
            account: Account::from(vote.voter).encode_account(),
            signature: vote.signature.encode_hex(),
            sequence: vote.timestamp().to_string(),
            timestamp: vote.timestamp().to_string(),
            duration: vote.duration_bits().to_string(),
            blocks: vote.hashes.iter().map(|h| h.to_string()).collect(),
            vote_type: match result {
                Ok(()) => "vote",
                Err(e) => e.as_str(),
            }
            .to_string(),
        },
    )
}

#[derive(Serialize, Deserialize)]
pub struct VoteReceived {
    pub account: String,
    pub signature: String,
    pub sequence: String,
    pub timestamp: String,
    pub duration: String,
    pub blocks: Vec<String>,
    #[serde(rename = "type")]
    pub vote_type: String,
}

pub struct NodeEventProcessor {
    server: Arc<WebsocketListener>,
    ledger: LedgerQueryHandle,
}

impl NodeEventHandler for NodeEventProcessor {
    fn handle(&mut self, event: &NodeEvent) {
        match event {
            NodeEvent::ElectionStarted(hash) => {
                if self.server.any_subscriber(Topic::StartedElection) {
                    self.server.broadcast(&started_election(hash));
                }
            }
            NodeEvent::ElectionStopped(hash) => {
                if self.server.any_subscriber(Topic::StoppedElection) {
                    self.server.broadcast(&stopped_election(hash));
                }
            }
            NodeEvent::BlockConfirmed(block, election) => {
                let amount = self.ledger.block_amount_for(block).unwrap_or_default();

                self.server.broadcast_confirmation(block, &amount, election);
            }
            NodeEvent::VoteProcessed(vote, vote_code) => {
                if self.server.any_subscriber(Topic::Vote) {
                    self.server.broadcast(&vote_received(vote, *vote_code));
                }
            }
            NodeEvent::BlocksProcessed(results) => {
                if self.server.any_subscriber(Topic::NewUnconfirmedBlock) {
                    for result in results {
                        if result.status.is_ok() {
                            let block = result.saved_block.as_ref().unwrap();
                            self.server.broadcast(&new_block_arrived_message(block));
                        }
                    }
                }
            }
        }
    }
}
