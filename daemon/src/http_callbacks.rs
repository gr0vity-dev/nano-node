use std::sync::Arc;

use serde::Serialize;
use tracing::error;

use rsnano_node::{
    NodeEvent, NodeEventHandler,
    consensus::election::{ConfirmationType, ConfirmedElection},
    handles::LedgerQueryHandle,
};
use rsnano_nullable_http_client::{HttpClient, Url};
use rsnano_types::{Amount, BlockType, SavedBlock};
use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};

/// Performs an HTTP callback to a configured endpoint
/// if a block is confirmed
pub(crate) struct HttpCallbacks {
    pub runtime: tokio::runtime::Handle,
    pub stats: Arc<Stats>,
    pub ledger: LedgerQueryHandle,
    pub callback_url: Url,
}

impl HttpCallbacks {
    pub fn execute(&self, election: &ConfirmedElection, block: &SavedBlock, amount: Amount) {
        let block = block.clone();
        if election.confirmation_type == ConfirmationType::ActiveConfirmedQuorum
            || election.confirmation_type == ConfirmationType::ActiveConfirmationHeight
        {
            let url = self.callback_url.clone();
            let stats = self.stats.clone();

            // TODO use a mpsc queue and a single async task for processing the queue
            // to avoid overload and out of order delivery
            // TODO warn if backlog is large (see nano_node)
            self.runtime.spawn(async move {
                let message = RpcCallbackMessage {
                    account: block.account().encode_account(),
                    hash: block.hash().encode_hex(),
                    block: (*block).clone().into(),
                    amount: amount.to_string_dec(),
                    sub_type: if block.block_type() == BlockType::State {
                        Some(block.subtype().as_str())
                    } else {
                        None
                    },
                    is_send: if block.block_type() == BlockType::State && block.is_send() {
                        Some("true")
                    } else {
                        None
                    },
                };

                let http_client = HttpClient::new();
                match http_client.post_json(url.clone(), &message).await {
                    Ok(response) => {
                        if response.status().is_success() {
                            stats.inc_dir(
                                StatType::HttpCallbacks,
                                DetailType::Initiate,
                                Direction::Out,
                            );
                        } else {
                            error!(
                                "Callback to {} failed [status: {:?}]",
                                url,
                                response.status()
                            );
                            stats.inc_dir(
                                StatType::Error,
                                DetailType::HttpCallback,
                                Direction::Out,
                            );
                        }
                    }
                    Err(e) => {
                        error!("Unable to send callback: {} ({})", url, e);
                        stats.inc_dir(StatType::Error, DetailType::HttpCallback, Direction::Out);
                    }
                }
            });
        }
    }
}

impl NodeEventHandler for HttpCallbacks {
    fn handle(&mut self, event: &NodeEvent) {
        if let NodeEvent::BlockConfirmed(block, status) = event {
            let amount = self.ledger.block_amount_for(block).unwrap_or_default();
            self.execute(status, block, amount)
        }
    }
}

#[derive(Serialize)]
struct RpcCallbackMessage {
    account: String,
    hash: String,
    block: serde_json::Value,
    amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_send: Option<&'static str>,
}
