use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UncementedBlocksArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_accounts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_blocks_per_account: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UncementedBlocksResponse {
    pub cache_count: String,
    pub store_count: String,
    pub confirmed_count: String,
    pub cache_inserts: String,
    pub cache_rollbacks: String,
    pub total_uncemented: String,
    pub accounts: Vec<UncementedAccountStatus>,
    pub insert_sources: Vec<UncementedInsertSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UncementedAccountStatus {
    pub account: String,
    pub head: String,
    pub confirmed_frontier: String,
    pub missing_count: String,
    pub sample_hashes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UncementedInsertSource {
    pub source: String,
    pub inserts: String,
}
