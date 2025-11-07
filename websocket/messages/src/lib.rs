#[macro_use]
extern crate num_derive;

#[macro_use]
extern crate strum_macros;

use std::{fmt::Debug, hash::Hash, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rsnano_types::{BlockHash, DifficultyV1, SavedBlock, WorkNonce, milliseconds_since_epoch};
use rsnano_work_validation::WorkThresholds;

#[derive(
    Clone, Copy, FromPrimitive, PartialEq, Eq, Hash, Serialize, Debug, Deserialize, IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Topic {
    Invalid = 0,
    /// Acknowledgement of prior incoming message
    Ack,
    /// A confirmation message
    Confirmation,
    StartedElection,
    /// Stopped election message (dropped elections due to bounding or block lost the elections)
    StoppedElection,
    /// A vote message
    Vote,
    /// Work generation message
    Work,
    /// A bootstrap message
    Bootstrap,
    /// A telemetry message
    Telemetry,
    /// New block arrival message
    NewUnconfirmedBlock,
}

#[derive(Serialize, Deserialize)]
pub struct Request<'a> {
    pub action: Option<&'a str>,
    pub topic: Option<&'a str>,
    #[serde(default)]
    pub ack: bool,
    pub id: Option<&'a str>,
    pub options: Option<Value>,
}

#[derive(Serialize, Clone, Debug, Deserialize)]
pub struct MessageEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<Topic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<BlockHash>,
    pub time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
}

impl MessageEnvelope {
    pub fn new(topic: Topic, message: impl Serialize) -> Self {
        Self {
            id: None,
            ack: None,
            topic: Some(topic),
            hash: None,
            time: milliseconds_since_epoch().to_string(),
            message: Some(serde_json::to_value(message).expect("could not serialize message")),
        }
    }

    pub fn new_ack(id: Option<String>, action: String) -> Self {
        Self {
            id,
            topic: None,
            ack: Some(action),
            hash: None,
            time: milliseconds_since_epoch().to_string(),
            message: None,
        }
    }
}

#[derive(Serialize)]
struct WorkGeneration<'a> {
    success: &'a str,
    reason: &'a str,
    duration: String,
    request: WorkRequest<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<WorkResult>,
    bad_peers: Vec<String>,
}

#[derive(Serialize)]
struct WorkRequest<'a> {
    version: &'a str,
    hash: String,
    difficulty: String,
    multiplier: String,
}

#[derive(Serialize)]
struct WorkResult {
    source: String,
    work: String,
    difficulty: String,
    multiplier: String,
}

pub fn work_generation_message(
    root: &BlockHash,
    work: WorkNonce,
    difficulty: u64,
    publish_threshold: u64,
    duration: Duration,
    peer: &str,
    bad_peers: &[String],
    completed: bool,
    cancelled: bool,
) -> MessageEnvelope {
    let request_multiplier = DifficultyV1::to_multiplier(difficulty, publish_threshold);
    let request = WorkRequest {
        version: "work_1",
        hash: root.to_string(),
        difficulty: format!("{:016x}", difficulty),
        multiplier: format!("{:.10}", request_multiplier),
    };

    let result = if completed {
        let result_difficulty = WorkThresholds::publish_full().difficulty(&root.into(), work);

        let result_multiplier = DifficultyV1::to_multiplier(result_difficulty, publish_threshold);

        Some(WorkResult {
            source: peer.to_string(),
            work: work.to_string(),
            difficulty: format!("{:016x}", result_difficulty),
            multiplier: format!("{:.10}", result_multiplier),
        })
    } else {
        None
    };

    let bad_peers = bad_peers.iter().cloned().collect();
    MessageEnvelope::new(
        Topic::Work,
        WorkGeneration {
            success: if completed { "true" } else { "false" },
            reason: if completed {
                ""
            } else if cancelled {
                "cancelled"
            } else {
                "failure"
            },
            duration: duration.as_millis().to_string(),
            request,
            result,
            bad_peers,
        },
    )
}

pub fn to_topic(topic: impl AsRef<str>) -> Topic {
    match topic.as_ref() {
        "confirmation" => Topic::Confirmation,
        "started_election" => Topic::StartedElection,
        "stopped_election" => Topic::StoppedElection,
        "vote" => Topic::Vote,
        "ack" => Topic::Ack,
        "work" => Topic::Work,
        "bootstrap" => Topic::Bootstrap,
        "telemetry" => Topic::Telemetry,
        "new_unconfirmed_block" => Topic::NewUnconfirmedBlock,
        _ => Topic::Invalid,
    }
}

pub fn new_block_arrived_message(block: &SavedBlock) -> MessageEnvelope {
    let json_block: serde_json::Value = block.clone().into();
    let mut result = MessageEnvelope::new(Topic::NewUnconfirmedBlock, json_block);
    result.hash = Some(block.hash());
    result
}

#[derive(Serialize, Deserialize, Default)]
pub struct ConfirmationJsonOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_block: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_election_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_election_info_with_votes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_linked_account: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_sideband_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_local_accounts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct JsonSideband {
    pub height: String,
    pub local_timestamp: String,
}

#[derive(Serialize, Deserialize)]
pub struct BlockConfirmed {
    pub account: String,
    pub amount: String,
    pub hash: String,
    pub confirmation_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub election_info: Option<ElectionInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sideband: Option<JsonSideband>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_account: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ElectionInfo {
    pub duration: String,
    pub time: String,
    pub tally: String,
    #[serde(rename = "final")]
    pub final_tally: String,
    pub blocks: String,
    pub voters: String,
    pub request_count: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<Vec<JsonVoteSummary>>,
}

#[derive(Serialize, Deserialize)]
pub struct JsonVoteSummary {
    pub representative: String,
    pub timestamp: String,
    pub hash: String,
    pub weight: String,
}
