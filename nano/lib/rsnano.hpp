#ifndef rs_nano_bindings_hpp
#define rs_nano_bindings_hpp

/* Warning, this file is autogenerated by cbindgen. Don't modify this manually. */

#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <new>
#include <ostream>

namespace rsnano
{
static const uintptr_t SignatureChecker_BATCH_SIZE = 256;

struct BandwidthLimiterHandle;

struct BlockArrivalHandle;

struct BlockHandle;

struct BlockProcessorHandle;

struct BlockUniquerHandle;

struct BootstrapAttemptHandle;

struct BootstrapInitiatorHandle;

struct EpochsHandle;

struct LedgerHandle;

struct LocalVoteHistoryHandle;

struct LocalVotesResultHandle;

struct LockHandle;

struct SignatureCheckerHandle;

struct StateBlockSignatureVerificationHandle;

struct StateBlockSignatureVerificationResultHandle;

struct StringHandle;

struct UncheckedInfoHandle;

struct VoteHandle;

struct VoteHashesHandle;

struct VoteUniquerHandle;

struct BlockDetailsDto
{
	uint8_t epoch;
	bool is_send;
	bool is_receive;
	bool is_epoch;
};

struct BlockSidebandDto
{
	uint64_t height;
	uint64_t timestamp;
	uint8_t successor[32];
	uint8_t account[32];
	uint8_t balance[16];
	BlockDetailsDto details;
	uint8_t source_epoch;
};

struct StringDto
{
	StringHandle * handle;
	const char * value;
};

struct WorkThresholdsDto
{
	uint64_t epoch_1;
	uint64_t epoch_2;
	uint64_t epoch_2_receive;
	uint64_t base;
	uint64_t entry;
};

struct NetworkConstantsDto
{
	uint16_t current_network;
	WorkThresholdsDto work;
	uint32_t principal_weight_factor;
	uint16_t default_node_port;
	uint16_t default_rpc_port;
	uint16_t default_ipc_port;
	uint16_t default_websocket_port;
	uint32_t request_interval_ms;
	int64_t cleanup_period_s;
	int64_t idle_timeout_s;
	int64_t sync_cookie_cutoff_s;
	int64_t bootstrap_interval_s;
	uintptr_t max_peers_per_ip;
	uintptr_t max_peers_per_subnetwork;
	int64_t peer_dump_interval_s;
	uint8_t protocol_version;
	uint8_t protocol_version_min;
	uintptr_t ipv6_subnetwork_prefix_for_limiting;
	int64_t silent_connection_tolerance_time_s;
};

struct BootstrapConstantsDto
{
	uint32_t lazy_max_pull_blocks;
	uint32_t lazy_min_pull_blocks;
	uint32_t frontier_retry_limit;
	uint32_t lazy_retry_limit;
	uint32_t lazy_destinations_retry_limit;
	int64_t gap_cache_bootstrap_start_interval_ms;
	uint32_t default_frontiers_age_seconds;
};

using AlwaysLogCallback = void (*) (void *, const uint8_t *, uintptr_t);

using Blake2BFinalCallback = int32_t (*) (void *, void *, uintptr_t);

using Blake2BInitCallback = int32_t (*) (void *, uintptr_t);

using Blake2BUpdateCallback = int32_t (*) (void *, const void *, uintptr_t);

using BootstrapInitiatorClearPullsCallback = void (*) (void *, uint64_t);

using BlockProcessorAddCallback = void (*) (void *, UncheckedInfoHandle *);

using InAvailCallback = uintptr_t (*) (void *, int32_t *);

using LedgerBlockOrPrunedExistsCallback = bool (*) (void *, const uint8_t *);

struct MessageDto
{
	uint8_t topic;
	void * contents;
};

using ListenerBroadcastCallback = bool (*) (void *, const MessageDto *);

using PropertyTreePutStringCallback = void (*) (void *, const char *, uintptr_t, const char *, uintptr_t);

using PropertyTreePushBackCallback = void (*) (void *, const char *, const void *);

using PropertyTreeCreateTreeCallback = void * (*)();

using PropertyTreeDestroyTreeCallback = void (*) (void *);

using PropertyTreeGetStringCallback = int32_t (*) (const void *, const char *, uintptr_t, char *, uintptr_t);

using PropertyTreePutU64Callback = void (*) (void *, const char *, uintptr_t, uint64_t);

using ReadBytesCallback = int32_t (*) (void *, uint8_t *, uintptr_t);

using ReadU8Callback = int32_t (*) (void *, uint8_t *);

using TomlArrayPutStrCallback = void (*) (void *, const uint8_t *, uintptr_t);

using TomlCreateArrayCallback = void * (*)(void *, const uint8_t *, uintptr_t, const uint8_t *, uintptr_t);

using TomlCreateConfigCallback = void * (*)();

using TomlDropArrayCallback = void (*) (void *);

using TomlDropConfigCallback = void (*) (void *);

using TomlPutBoolCallback = int32_t (*) (void *, const uint8_t *, uintptr_t, bool, const uint8_t *, uintptr_t);

using TomlPutChildCallback = void (*) (void *, const uint8_t *, uintptr_t, void *);

using TomlPutF64Callback = int32_t (*) (void *, const uint8_t *, uintptr_t, double, const uint8_t *, uintptr_t);

using TomlPutI64Callback = int32_t (*) (void *, const uint8_t *, uintptr_t, int64_t, const uint8_t *, uintptr_t);

using TomlPutStrCallback = int32_t (*) (void *, const uint8_t *, uintptr_t, const uint8_t *, uintptr_t, const uint8_t *, uintptr_t);

using TomlPutU64Callback = int32_t (*) (void *, const uint8_t *, uintptr_t, uint64_t, const uint8_t *, uintptr_t);

using TryLogCallback = bool (*) (void *, const uint8_t *, uintptr_t);

using WriteBytesCallback = int32_t (*) (void *, const uint8_t *, uintptr_t);

using WriteU8Callback = int32_t (*) (void *, uint8_t);

struct ChangeBlockDto
{
	uint64_t work;
	uint8_t signature[64];
	uint8_t previous[32];
	uint8_t representative[32];
};

struct ChangeBlockDto2
{
	uint8_t previous[32];
	uint8_t representative[32];
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint64_t work;
};

struct PeerDto
{
	uint8_t address[128];
	uintptr_t address_len;
	uint16_t port;
};

struct LoggingDto
{
	bool ledger_logging_value;
	bool ledger_duplicate_logging_value;
	bool ledger_rollback_logging_value;
	bool vote_logging_value;
	bool rep_crawler_logging_value;
	bool election_fork_tally_logging_value;
	bool election_expiration_tally_logging_value;
	bool network_logging_value;
	bool network_timeout_logging_value;
	bool network_message_logging_value;
	bool network_publish_logging_value;
	bool network_packet_logging_value;
	bool network_keepalive_logging_value;
	bool network_node_id_handshake_logging_value;
	bool network_telemetry_logging_value;
	bool network_rejected_logging_value;
	bool node_lifetime_tracing_value;
	bool insufficient_work_logging_value;
	bool log_ipc_value;
	bool bulk_pull_logging_value;
	bool work_generation_time_value;
	bool upnp_details_logging_value;
	bool timing_logging_value;
	bool active_update_value;
	bool log_to_cerr_value;
	bool flush;
	uintptr_t max_size;
	uintptr_t rotation_size;
	bool stable_log_filename;
	int64_t min_time_between_log_output_ms;
	bool single_line_record_value;
	bool election_result_logging_value;
};

struct WebsocketConfigDto
{
	bool enabled;
	uint16_t port;
	uint8_t address[128];
	uintptr_t address_len;
};

struct IpcConfigTransportDto
{
	bool enabled;
	bool allow_unsafe;
	uintptr_t io_timeout;
	int64_t io_threads;
};

struct IpcConfigDto
{
	IpcConfigTransportDto domain_transport;
	uint8_t domain_path[512];
	uintptr_t domain_path_len;
	IpcConfigTransportDto tcp_transport;
	NetworkConstantsDto tcp_network_constants;
	uint16_t tcp_port;
	bool flatbuffers_skip_unexpected_fields_in_json;
	bool flatbuffers_verify_buffers;
};

struct TxnTrackingConfigDto
{
	bool enable;
	int64_t min_read_txn_time_ms;
	int64_t min_write_txn_time_ms;
	bool ignore_writes_below_block_processor_max_time;
};

struct StatConfigDto
{
	bool sampling_enabled;
	uintptr_t capacity;
	uintptr_t interval;
	uintptr_t log_interval_samples;
	uintptr_t log_interval_counters;
	uintptr_t log_rotation_count;
	bool log_headers;
	uint8_t log_counters_filename[128];
	uintptr_t log_counters_filename_len;
	uint8_t log_samples_filename[128];
	uintptr_t log_samples_filename_len;
};

struct RocksDbConfigDto
{
	bool enable;
	uint8_t memory_multiplier;
	uint32_t io_threads;
};

struct LmdbConfigDto
{
	uint8_t sync;
	uint32_t max_databases;
	uintptr_t map_size;
};

struct NodeConfigDto
{
	uint16_t peering_port;
	bool peering_port_defined;
	uint32_t bootstrap_fraction_numerator;
	uint8_t receive_minimum[16];
	uint8_t online_weight_minimum[16];
	uint32_t election_hint_weight_percent;
	uint32_t password_fanout;
	uint32_t io_threads;
	uint32_t network_threads;
	uint32_t work_threads;
	uint32_t signature_checker_threads;
	bool enable_voting;
	uint32_t bootstrap_connections;
	uint32_t bootstrap_connections_max;
	uint32_t bootstrap_initiator_threads;
	uint32_t bootstrap_frontier_request_count;
	int64_t block_processor_batch_max_time_ms;
	bool allow_local_peers;
	uint8_t vote_minimum[16];
	int64_t vote_generator_delay_ms;
	uint32_t vote_generator_threshold;
	int64_t unchecked_cutoff_time_s;
	int64_t tcp_io_timeout_s;
	int64_t pow_sleep_interval_ns;
	uint8_t external_address[128];
	uintptr_t external_address_len;
	uint16_t external_port;
	uint32_t tcp_incoming_connections_max;
	bool use_memory_pools;
	uintptr_t confirmation_history_size;
	uintptr_t active_elections_size;
	uintptr_t bandwidth_limit;
	double bandwidth_limit_burst_ratio;
	int64_t conf_height_processor_batch_min_time_ms;
	bool backup_before_upgrade;
	double max_work_generate_multiplier;
	uint8_t frontiers_confirmation;
	uint32_t max_queued_requests;
	uint8_t rep_crawler_weight_minimum[16];
	PeerDto work_peers[5];
	uintptr_t work_peers_count;
	PeerDto secondary_work_peers[5];
	uintptr_t secondary_work_peers_count;
	PeerDto preconfigured_peers[5];
	uintptr_t preconfigured_peers_count;
	uint8_t preconfigured_representatives[10][32];
	uintptr_t preconfigured_representatives_count;
	int64_t max_pruning_age_s;
	uint64_t max_pruning_depth;
	uint8_t callback_address[128];
	uintptr_t callback_address_len;
	uint16_t callback_port;
	uint8_t callback_target[128];
	uintptr_t callback_target_len;
	LoggingDto logging;
	WebsocketConfigDto websocket_config;
	IpcConfigDto ipc_config;
	TxnTrackingConfigDto diagnostics_config;
	StatConfigDto stat_config;
	RocksDbConfigDto rocksdb_config;
	LmdbConfigDto lmdb_config;
};

struct OpenclConfigDto
{
	uint32_t platform;
	uint32_t device;
	uint32_t threads;
};

struct NodePowServerConfigDto
{
	bool enable;
	uint8_t pow_server_path[128];
	uintptr_t pow_server_path_len;
};

struct NodeRpcConfigDto
{
	uint8_t rpc_path[512];
	uintptr_t rpc_path_length;
	bool enable_child_process;
	bool enable_sign_hash;
};

struct DaemonConfigDto
{
	bool rpc_enable;
	NodeConfigDto node;
	OpenclConfigDto opencl;
	bool opencl_enable;
	NodePowServerConfigDto pow_server;
	NodeRpcConfigDto rpc;
};

struct LedgerConstantsDto
{
	WorkThresholdsDto work;
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint8_t nano_beta_account[32];
	uint8_t nano_live_account[32];
	uint8_t nano_test_account[32];
	BlockHandle * nano_dev_genesis;
	BlockHandle * nano_beta_genesis;
	BlockHandle * nano_live_genesis;
	BlockHandle * nano_test_genesis;
	BlockHandle * genesis;
	uint8_t genesis_amount[16];
	uint8_t burn_account[32];
	uint8_t nano_dev_final_votes_canary_account[32];
	uint8_t nano_beta_final_votes_canary_account[32];
	uint8_t nano_live_final_votes_canary_account[32];
	uint8_t nano_test_final_votes_canary_account[32];
	uint8_t final_votes_canary_account[32];
	uint64_t nano_dev_final_votes_canary_height;
	uint64_t nano_beta_final_votes_canary_height;
	uint64_t nano_live_final_votes_canary_height;
	uint64_t nano_test_final_votes_canary_height;
	uint64_t final_votes_canary_height;
	uint8_t epoch_1_signer[32];
	uint8_t epoch_1_link[32];
	uint8_t epoch_2_signer[32];
	uint8_t epoch_2_link[32];
};

struct VotingConstantsDto
{
	uintptr_t max_cache;
	int64_t delay_s;
};

struct NodeConstantsDto
{
	int64_t backup_interval_m;
	int64_t search_pending_interval_s;
	int64_t unchecked_cleaning_interval_m;
	int64_t process_confirmed_interval_ms;
	uint64_t max_weight_samples;
	uint64_t weight_period;
};

struct PortmappingConstantsDto
{
	int64_t lease_duration_s;
	int64_t health_check_period_s;
};

struct NetworkParamsDto
{
	uint32_t kdf_work;
	WorkThresholdsDto work;
	NetworkConstantsDto network;
	LedgerConstantsDto ledger;
	VotingConstantsDto voting;
	NodeConstantsDto node;
	PortmappingConstantsDto portmapping;
	BootstrapConstantsDto bootstrap;
};

struct LocalVotesResult
{
	uintptr_t count;
	VoteHandle * const * votes;
	LocalVotesResultHandle * handle;
};

struct OpenBlockDto
{
	uint64_t work;
	uint8_t signature[64];
	uint8_t source[32];
	uint8_t representative[32];
	uint8_t account[32];
};

struct OpenBlockDto2
{
	uint8_t source[32];
	uint8_t representative[32];
	uint8_t account[32];
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint64_t work;
};

struct ReceiveBlockDto
{
	uint64_t work;
	uint8_t signature[64];
	uint8_t previous[32];
	uint8_t source[32];
};

struct ReceiveBlockDto2
{
	uint8_t previous[32];
	uint8_t source[32];
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint64_t work;
};

struct RpcProcessConfigDto
{
	uint32_t io_threads;
	uint8_t ipc_address[128];
	uintptr_t ipc_address_len;
	uint16_t ipc_port;
	uint32_t num_ipc_connections;
};

struct RpcConfigDto
{
	uint8_t address[128];
	uintptr_t address_len;
	uint16_t port;
	bool enable_control;
	uint8_t max_json_depth;
	uint64_t max_request_size;
	bool rpc_log;
	RpcProcessConfigDto rpc_process;
};

struct SendBlockDto
{
	uint8_t previous[32];
	uint8_t destination[32];
	uint8_t balance[16];
	uint8_t signature[64];
	uint64_t work;
};

struct SendBlockDto2
{
	uint8_t previous[32];
	uint8_t destination[32];
	uint8_t balance[16];
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint64_t work;
};

struct SignatureCheckSetDto
{
	uintptr_t size;
	const uint8_t * const * messages;
	const uintptr_t * message_lengths;
	const uint8_t * const * pub_keys;
	const uint8_t * const * signatures;
	int32_t * verifications;
};

struct StateBlockDto
{
	uint8_t signature[64];
	uint8_t account[32];
	uint8_t previous[32];
	uint8_t representative[32];
	uint8_t link[32];
	uint8_t balance[16];
	uint64_t work;
};

struct StateBlockDto2
{
	uint8_t account[32];
	uint8_t previous[32];
	uint8_t representative[32];
	uint8_t link[32];
	uint8_t balance[16];
	uint8_t priv_key[32];
	uint8_t pub_key[32];
	uint64_t work;
};

struct StateBlockSignatureVerificationValueDto
{
	BlockHandle * block;
	uint8_t account[32];
	uint8_t verification;
};

using TransitionInactiveCallback = void (*) (void *);

struct StateBlockSignatureVerificationResultDto
{
	const uint8_t (*hashes)[32];
	const uint8_t (*signatures)[64];
	const int32_t * verifications;
	const StateBlockSignatureVerificationValueDto * items;
	uintptr_t size;
	StateBlockSignatureVerificationResultHandle * handle;
};

using StateBlockVerifiedCallback = void (*) (void *, const StateBlockSignatureVerificationResultDto *);

struct VoteHashesDto
{
	VoteHashesHandle * handle;
	uintptr_t count;
	const uint8_t (*hashes)[32];
};

extern "C" {

int32_t rsn_account_decode (const char * input, uint8_t (*result)[32]);

void rsn_account_encode (const uint8_t (*bytes)[32], uint8_t (*result)[65]);

BandwidthLimiterHandle * rsn_bandwidth_limiter_create (double limit_burst_ratio, uintptr_t limit);

void rsn_bandwidth_limiter_destroy (BandwidthLimiterHandle * limiter);

int32_t rsn_bandwidth_limiter_reset (const BandwidthLimiterHandle * limiter,
double limit_burst_ratio,
uintptr_t limit);

bool rsn_bandwidth_limiter_should_drop (const BandwidthLimiterHandle * limiter,
uintptr_t message_size,
int32_t * result);

bool rsn_block_arrival_add (BlockArrivalHandle * handle, const uint8_t * hash);

BlockArrivalHandle * rsn_block_arrival_create ();

void rsn_block_arrival_destroy (BlockArrivalHandle * handle);

bool rsn_block_arrival_recent (BlockArrivalHandle * handle, const uint8_t * hash);

uintptr_t rsn_block_arrival_size (BlockArrivalHandle * handle);

uintptr_t rsn_block_arrival_size_of_element (BlockArrivalHandle * handle);

BlockHandle * rsn_block_clone (const BlockHandle * handle);

void rsn_block_destroy (BlockHandle * handle);

int32_t rsn_block_details_create (uint8_t epoch,
bool is_send,
bool is_receive,
bool is_epoch,
BlockDetailsDto * result);

int32_t rsn_block_details_deserialize (BlockDetailsDto * dto, void * stream);

int32_t rsn_block_details_serialize (const BlockDetailsDto * dto, void * stream);

bool rsn_block_equals (const BlockHandle * a, const BlockHandle * b);

void rsn_block_full_hash (const BlockHandle * handle, uint8_t * hash);

BlockHandle * rsn_block_handle_clone (const BlockHandle * handle);

bool rsn_block_has_sideband (const BlockHandle * block);

void rsn_block_hash (const BlockHandle * handle, uint8_t (*hash)[32]);

void rsn_block_previous (const BlockHandle * handle, uint8_t (*result)[32]);

BlockProcessorHandle * rsn_block_processor_create (void * handle);

void rsn_block_processor_destroy (BlockProcessorHandle * handle);

const void * rsn_block_rust_data_pointer (const BlockHandle * handle);

int32_t rsn_block_serialize (BlockHandle * handle, void * stream);

int32_t rsn_block_serialize_json (const BlockHandle * handle, void * ptree);

uintptr_t rsn_block_serialized_size (uint8_t block_type);

int32_t rsn_block_sideband (const BlockHandle * block, BlockSidebandDto * sideband);

int32_t rsn_block_sideband_deserialize (BlockSidebandDto * dto, void * stream, uint8_t block_type);

int32_t rsn_block_sideband_serialize (const BlockSidebandDto * dto, void * stream, uint8_t block_type);

int32_t rsn_block_sideband_set (BlockHandle * block, const BlockSidebandDto * sideband);

uintptr_t rsn_block_sideband_size (uint8_t block_type, int32_t * result);

void rsn_block_signature (const BlockHandle * handle, uint8_t (*result)[64]);

void rsn_block_signature_set (BlockHandle * handle, const uint8_t (*signature)[64]);

uint8_t rsn_block_type (const BlockHandle * handle);

BlockUniquerHandle * rsn_block_uniquer_create ();

void rsn_block_uniquer_destroy (BlockUniquerHandle * handle);

uintptr_t rsn_block_uniquer_size (const BlockUniquerHandle * handle);

BlockHandle * rsn_block_uniquer_unique (BlockUniquerHandle * handle, BlockHandle * block);

uint64_t rsn_block_work (const BlockHandle * handle);

void rsn_block_work_set (BlockHandle * handle, uint64_t work);

const char * rsn_bootstrap_attempt_bootstrap_mode (const BootstrapAttemptHandle * handle,
uintptr_t * len);

BootstrapAttemptHandle * rsn_bootstrap_attempt_create (void * logger,
void * websocket_server,
const BlockProcessorHandle * block_processor,
const BootstrapInitiatorHandle * bootstrap_initiator,
const LedgerHandle * ledger,
const char * id,
uint8_t mode,
uint64_t incremental_id);

void rsn_bootstrap_attempt_destroy (BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_id (const BootstrapAttemptHandle * handle, StringDto * result);

LockHandle * rsn_bootstrap_attempt_lock (BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_notifiy_all (BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_notifiy_one (BootstrapAttemptHandle * handle);

bool rsn_bootstrap_attempt_process_block (const BootstrapAttemptHandle * handle,
const BlockHandle * block,
const uint8_t * known_account,
uint64_t pull_blocks_processed,
uint32_t max_blocks,
bool block_expected,
uint32_t retry_limit);

bool rsn_bootstrap_attempt_should_log (const BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_stop (BootstrapAttemptHandle * handle);

uint64_t rsn_bootstrap_attempt_total_blocks (const BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_total_blocks_inc (const BootstrapAttemptHandle * handle);

void rsn_bootstrap_attempt_unlock (LockHandle * handle);

void rsn_bootstrap_attempt_wait (BootstrapAttemptHandle * handle, LockHandle * lck);

void rsn_bootstrap_attempt_wait_for (BootstrapAttemptHandle * handle,
LockHandle * lck,
uint64_t timeout_millis);

int32_t rsn_bootstrap_constants_create (const NetworkConstantsDto * network_constants,
BootstrapConstantsDto * dto);

BootstrapInitiatorHandle * rsn_bootstrap_initiator_create (void * handle);

void rsn_bootstrap_initiator_destroy (BootstrapInitiatorHandle * handle);

void rsn_callback_always_log (AlwaysLogCallback f);

void rsn_callback_blake2b_final (Blake2BFinalCallback f);

void rsn_callback_blake2b_init (Blake2BInitCallback f);

void rsn_callback_blake2b_update (Blake2BUpdateCallback f);

void rsn_callback_block_bootstrap_initiator_clear_pulls (BootstrapInitiatorClearPullsCallback f);

void rsn_callback_block_processor_add (BlockProcessorAddCallback f);

void rsn_callback_in_avail (InAvailCallback f);

void rsn_callback_ledger_block_or_pruned_exists (LedgerBlockOrPrunedExistsCallback f);

void rsn_callback_listener_broadcast (ListenerBroadcastCallback f);

void rsn_callback_property_tree_add (PropertyTreePutStringCallback f);

void rsn_callback_property_tree_add_child (PropertyTreePushBackCallback f);

void rsn_callback_property_tree_create (PropertyTreeCreateTreeCallback f);

void rsn_callback_property_tree_destroy (PropertyTreeDestroyTreeCallback f);

void rsn_callback_property_tree_get_string (PropertyTreeGetStringCallback f);

void rsn_callback_property_tree_push_back (PropertyTreePushBackCallback f);

void rsn_callback_property_tree_put_string (PropertyTreePutStringCallback f);

void rsn_callback_property_tree_put_u64 (PropertyTreePutU64Callback f);

void rsn_callback_read_bytes (ReadBytesCallback f);

void rsn_callback_read_u8 (ReadU8Callback f);

void rsn_callback_toml_array_put_str (TomlArrayPutStrCallback f);

void rsn_callback_toml_create_array (TomlCreateArrayCallback f);

void rsn_callback_toml_create_config (TomlCreateConfigCallback f);

void rsn_callback_toml_drop_array (TomlDropArrayCallback f);

void rsn_callback_toml_drop_config (TomlDropConfigCallback f);

void rsn_callback_toml_put_bool (TomlPutBoolCallback f);

void rsn_callback_toml_put_child (TomlPutChildCallback f);

void rsn_callback_toml_put_f64 (TomlPutF64Callback f);

void rsn_callback_toml_put_i64 (TomlPutI64Callback f);

void rsn_callback_toml_put_str (TomlPutStrCallback f);

void rsn_callback_toml_put_u64 (TomlPutU64Callback f);

void rsn_callback_try_log (TryLogCallback f);

void rsn_callback_write_bytes (WriteBytesCallback f);

void rsn_callback_write_u8 (WriteU8Callback f);

BlockHandle * rsn_change_block_create (const ChangeBlockDto * dto);

BlockHandle * rsn_change_block_create2 (const ChangeBlockDto2 * dto);

BlockHandle * rsn_change_block_deserialize (void * stream);

BlockHandle * rsn_change_block_deserialize_json (const void * ptree);

void rsn_change_block_previous_set (BlockHandle * handle, const uint8_t (*source)[32]);

void rsn_change_block_representative (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_change_block_representative_set (BlockHandle * handle, const uint8_t (*representative)[32]);

uintptr_t rsn_change_block_size ();

int32_t rsn_daemon_config_create (DaemonConfigDto * dto, const NetworkParamsDto * network_params);

int32_t rsn_daemon_config_serialize_toml (const DaemonConfigDto * dto, void * toml);

BlockHandle * rsn_deserialize_block_json (const void * ptree);

uint64_t rsn_difficulty_from_multiplier (double multiplier, uint64_t base_difficulty);

double rsn_difficulty_to_multiplier (uint64_t difficulty, uint64_t base_difficulty);

void rsn_epochs_add (EpochsHandle * handle,
uint8_t epoch,
const uint8_t * signer,
const uint8_t * link);

EpochsHandle * rsn_epochs_create ();

void rsn_epochs_destroy (EpochsHandle * handle);

uint8_t rsn_epochs_epoch (const EpochsHandle * handle, const uint8_t * link);

bool rsn_epochs_is_epoch_link (const EpochsHandle * handle, const uint8_t * link);

void rsn_epochs_link (const EpochsHandle * handle, uint8_t epoch, uint8_t * link);

void rsn_epochs_signer (const EpochsHandle * handle, uint8_t epoch, uint8_t * signer);

void rsn_from_topic (uint8_t topic, StringDto * result);

void rsn_hardened_constants_get (uint8_t * not_an_account, uint8_t * random_128);

int32_t rsn_ipc_config_create (IpcConfigDto * dto, const NetworkConstantsDto * network_constants);

int32_t rsn_ledger_constants_create (LedgerConstantsDto * dto,
const WorkThresholdsDto * work,
uint16_t network);

LedgerHandle * rsn_ledger_create (void * handle);

void rsn_ledger_destroy (LedgerHandle * handle);

void rsn_lmdb_config_create (LmdbConfigDto * dto);

void rsn_local_vote_history_add (LocalVoteHistoryHandle * handle,
const uint8_t * root,
const uint8_t * hash,
const VoteHandle * vote);

void rsn_local_vote_history_container_info (LocalVoteHistoryHandle * handle,
uintptr_t * size,
uintptr_t * count);

LocalVoteHistoryHandle * rsn_local_vote_history_create (uintptr_t max_cache);

void rsn_local_vote_history_destroy (LocalVoteHistoryHandle * handle);

void rsn_local_vote_history_erase (LocalVoteHistoryHandle * handle, const uint8_t * root);

bool rsn_local_vote_history_exists (LocalVoteHistoryHandle * handle, const uint8_t * root);

uintptr_t rsn_local_vote_history_size (LocalVoteHistoryHandle * handle);

void rsn_local_vote_history_votes (LocalVoteHistoryHandle * handle,
const uint8_t * root,
const uint8_t * hash,
bool is_final,
LocalVotesResult * result);

void rsn_local_vote_history_votes_destroy (LocalVotesResultHandle * handle);

void rsn_logging_create (LoggingDto * dto);

void rsn_message_builder_bootstrap_exited (const char * id,
const char * mode,
uint64_t duration_s,
uint64_t total_blocks,
MessageDto * result);

void rsn_message_builder_bootstrap_started (const char * id, const char * mode, MessageDto * result);

uint16_t rsn_network_constants_active_network ();

void rsn_network_constants_active_network_set (uint16_t network);

int32_t rsn_network_constants_active_network_set_str (const char * network);

int64_t rsn_network_constants_cleanup_cutoff_s (const NetworkConstantsDto * dto);

int64_t rsn_network_constants_cleanup_period_half_ms (const NetworkConstantsDto * dto);

int32_t rsn_network_constants_create (NetworkConstantsDto * dto,
const WorkThresholdsDto * work,
uint16_t network);

bool rsn_network_constants_is_beta_network (const NetworkConstantsDto * dto);

bool rsn_network_constants_is_dev_network (const NetworkConstantsDto * dto);

bool rsn_network_constants_is_live_network (const NetworkConstantsDto * dto);

bool rsn_network_constants_is_test_network (const NetworkConstantsDto * dto);

int32_t rsn_network_params_create (NetworkParamsDto * dto, uint16_t network);

int32_t rsn_node_config_create (NodeConfigDto * dto,
uint16_t peering_port,
bool peering_port_defined,
const LoggingDto * logging,
const NetworkParamsDto * network_params);

int32_t rsn_node_config_serialize_toml (const NodeConfigDto * dto, void * toml);

int32_t rsn_node_constants_create (const NetworkConstantsDto * network_constants,
NodeConstantsDto * dto);

int32_t rsn_node_rpc_config_create (NodeRpcConfigDto * dto);

void rsn_open_block_account (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_open_block_account_set (BlockHandle * handle, const uint8_t (*account)[32]);

BlockHandle * rsn_open_block_create (const OpenBlockDto * dto);

BlockHandle * rsn_open_block_create2 (const OpenBlockDto2 * dto);

BlockHandle * rsn_open_block_deserialize (void * stream);

BlockHandle * rsn_open_block_deserialize_json (const void * ptree);

void rsn_open_block_representative (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_open_block_representative_set (BlockHandle * handle, const uint8_t (*representative)[32]);

uintptr_t rsn_open_block_size ();

void rsn_open_block_source (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_open_block_source_set (BlockHandle * handle, const uint8_t (*source)[32]);

int32_t rsn_portmapping_constants_create (const NetworkConstantsDto * network_constants,
PortmappingConstantsDto * dto);

BlockHandle * rsn_receive_block_create (const ReceiveBlockDto * dto);

BlockHandle * rsn_receive_block_create2 (const ReceiveBlockDto2 * dto);

BlockHandle * rsn_receive_block_deserialize (void * stream);

BlockHandle * rsn_receive_block_deserialize_json (const void * ptree);

void rsn_receive_block_previous_set (BlockHandle * handle, const uint8_t (*previous)[32]);

uintptr_t rsn_receive_block_size ();

void rsn_receive_block_source (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_receive_block_source_set (BlockHandle * handle, const uint8_t (*previous)[32]);

void rsn_remove_temporary_directories ();

void rsn_rocksdb_config_create (RocksDbConfigDto * dto);

int32_t rsn_rpc_config_create (RpcConfigDto * dto, const NetworkConstantsDto * network_constants);

int32_t rsn_rpc_config_create2 (RpcConfigDto * dto,
const NetworkConstantsDto * network_constants,
uint16_t port,
bool enable_control);

int32_t rsn_rpc_config_serialize_toml (const RpcConfigDto * dto, void * toml);

void rsn_send_block_balance (const BlockHandle * handle, uint8_t (*result)[16]);

void rsn_send_block_balance_set (BlockHandle * handle, const uint8_t (*balance)[16]);

BlockHandle * rsn_send_block_create (const SendBlockDto * dto);

BlockHandle * rsn_send_block_create2 (const SendBlockDto2 * dto);

BlockHandle * rsn_send_block_deserialize (void * stream);

BlockHandle * rsn_send_block_deserialize_json (const void * ptree);

void rsn_send_block_destination (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_send_block_destination_set (BlockHandle * handle, const uint8_t (*destination)[32]);

void rsn_send_block_previous_set (BlockHandle * handle, const uint8_t (*previous)[32]);

uintptr_t rsn_send_block_size ();

bool rsn_send_block_valid_predecessor (uint8_t block_type);

void rsn_send_block_zero (BlockHandle * handle);

void rsn_shared_block_enum_handle_destroy (BlockHandle * handle);

int32_t rsn_sign_message (const uint8_t (*priv_key)[32],
const uint8_t (*pub_key)[32],
const uint8_t * message,
uintptr_t len,
uint8_t (*signature)[64]);

uintptr_t rsn_signature_checker_batch_size ();

SignatureCheckerHandle * rsn_signature_checker_create (uintptr_t num_threads);

void rsn_signature_checker_destroy (SignatureCheckerHandle * handle);

void rsn_signature_checker_flush (const SignatureCheckerHandle * handle);

void rsn_signature_checker_stop (SignatureCheckerHandle * handle);

void rsn_signature_checker_verify (const SignatureCheckerHandle * handle,
SignatureCheckSetDto * check_set);

void rsn_stat_config_create (StatConfigDto * dto);

void rsn_state_block_account (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_state_block_account_set (BlockHandle * handle, const uint8_t (*source)[32]);

void rsn_state_block_balance (const BlockHandle * handle, uint8_t (*result)[16]);

void rsn_state_block_balance_set (BlockHandle * handle, const uint8_t (*balance)[16]);

BlockHandle * rsn_state_block_create (const StateBlockDto * dto);

BlockHandle * rsn_state_block_create2 (const StateBlockDto2 * dto);

BlockHandle * rsn_state_block_deserialize (void * stream);

BlockHandle * rsn_state_block_deserialize_json (const void * ptree);

void rsn_state_block_link (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_state_block_link_set (BlockHandle * handle, const uint8_t (*link)[32]);

void rsn_state_block_previous_set (BlockHandle * handle, const uint8_t (*source)[32]);

void rsn_state_block_representative (const BlockHandle * handle, uint8_t (*result)[32]);

void rsn_state_block_representative_set (BlockHandle * handle, const uint8_t (*representative)[32]);

void rsn_state_block_signature_verification_add (StateBlockSignatureVerificationHandle * handle,
const StateBlockSignatureVerificationValueDto * block);

StateBlockSignatureVerificationHandle * rsn_state_block_signature_verification_create (const SignatureCheckerHandle * checker,
const EpochsHandle * epochs,
void * logger,
bool timing_logging,
uintptr_t verification_size);

void rsn_state_block_signature_verification_destroy (StateBlockSignatureVerificationHandle * handle);

bool rsn_state_block_signature_verification_is_active (const StateBlockSignatureVerificationHandle * handle);

void rsn_state_block_signature_verification_result_destroy (StateBlockSignatureVerificationResultHandle * handle);

uintptr_t rsn_state_block_signature_verification_size (const StateBlockSignatureVerificationHandle * handle);

void rsn_state_block_signature_verification_stop (StateBlockSignatureVerificationHandle * handle);

void rsn_state_block_signature_verification_transition_inactive_callback (StateBlockSignatureVerificationHandle * handle,
TransitionInactiveCallback callback,
void * context);

void rsn_state_block_signature_verification_verified_callback (StateBlockSignatureVerificationHandle * handle,
StateBlockVerifiedCallback callback,
void * context);

uintptr_t rsn_state_block_size ();

void rsn_string_destroy (StringHandle * handle);

uint16_t rsn_test_node_port ();

uint8_t rsn_to_topic (const char * topic);

void rsn_txn_tracking_config_create (TxnTrackingConfigDto * dto);

void rsn_unchecked_info_account (const UncheckedInfoHandle * handle, uint8_t * result);

void rsn_unchecked_info_account_set (UncheckedInfoHandle * handle, const uint8_t * account);

BlockHandle * rsn_unchecked_info_block (const UncheckedInfoHandle * handle);

void rsn_unchecked_info_block_set (UncheckedInfoHandle * handle, BlockHandle * block);

UncheckedInfoHandle * rsn_unchecked_info_clone (const UncheckedInfoHandle * handle);

UncheckedInfoHandle * rsn_unchecked_info_create ();

UncheckedInfoHandle * rsn_unchecked_info_create2 (const BlockHandle * block,
const uint8_t * account,
uint8_t verified);

void rsn_unchecked_info_destroy (UncheckedInfoHandle * handle);

uint64_t rsn_unchecked_info_modified (const UncheckedInfoHandle * handle);

void rsn_unchecked_info_modified_set (UncheckedInfoHandle * handle, uint64_t modified);

uint8_t rsn_unchecked_info_verified (const UncheckedInfoHandle * handle);

void rsn_unchecked_info_verified_set (UncheckedInfoHandle * handle, uint8_t verified);

int32_t rsn_unique_path (uint16_t network, uint8_t * result, uintptr_t size);

bool rsn_using_rocksdb_in_tests ();

bool rsn_validate_batch (const uint8_t * const * messages,
const uintptr_t * message_lengths,
const uint8_t * const * public_keys,
const uint8_t * const * signatures,
uintptr_t num,
int32_t * valid);

bool rsn_validate_message (const uint8_t (*pub_key)[32],
const uint8_t * message,
uintptr_t len,
const uint8_t (*signature)[64]);

void rsn_vote_account (const VoteHandle * handle, uint8_t * result);

void rsn_vote_account_set (VoteHandle * handle, const uint8_t * account);

VoteHandle * rsn_vote_copy (const VoteHandle * handle);

VoteHandle * rsn_vote_create ();

VoteHandle * rsn_vote_create2 (const uint8_t * account,
const uint8_t * prv_key,
uint64_t timestamp,
uint8_t duration,
const uint8_t (*hashes)[32],
uintptr_t hash_count);

int32_t rsn_vote_deserialize (const VoteHandle * handle, void * stream);

void rsn_vote_destroy (VoteHandle * handle);

uint8_t rsn_vote_duration_bits (const VoteHandle * handle);

uint64_t rsn_vote_duration_ms (const VoteHandle * handle);

bool rsn_vote_equals (const VoteHandle * first, const VoteHandle * second);

void rsn_vote_full_hash (const VoteHandle * handle, uint8_t * result);

void rsn_vote_hash (const VoteHandle * handle, uint8_t * result);

VoteHashesDto rsn_vote_hashes (const VoteHandle * handle);

void rsn_vote_hashes_destroy (VoteHashesHandle * hashes);

void rsn_vote_hashes_set (VoteHandle * handle, const uint8_t (*hashes)[32], uintptr_t count);

StringDto rsn_vote_hashes_string (const VoteHandle * handle);

const void * rsn_vote_rust_data_pointer (const VoteHandle * handle);

int32_t rsn_vote_serialize (const VoteHandle * handle, void * stream);

void rsn_vote_serialize_json (const VoteHandle * handle, void * ptree);

void rsn_vote_signature (const VoteHandle * handle, uint8_t * result);

void rsn_vote_signature_set (VoteHandle * handle, const uint8_t * signature);

uint64_t rsn_vote_timestamp (const VoteHandle * handle);

uint64_t rsn_vote_timestamp_raw (const VoteHandle * handle);

void rsn_vote_timestamp_raw_set (VoteHandle * handle, uint64_t timestamp);

VoteUniquerHandle * rsn_vote_uniquer_create ();

void rsn_vote_uniquer_destroy (VoteUniquerHandle * handle);

uintptr_t rsn_vote_uniquer_size (const VoteUniquerHandle * handle);

VoteHandle * rsn_vote_uniquer_unique (VoteUniquerHandle * handle, VoteHandle * vote);

bool rsn_vote_validate (const VoteHandle * handle);

int32_t rsn_voting_constants_create (const NetworkConstantsDto * network_constants,
VotingConstantsDto * dto);

int32_t rsn_websocket_config_create (WebsocketConfigDto * dto, const NetworkConstantsDto * network);

void rsn_websocket_set_common_fields (MessageDto * message);

void rsn_work_thresholds_create (WorkThresholdsDto * dto,
uint64_t epoch_1,
uint64_t epoch_2,
uint64_t epoch_2_receive);

double rsn_work_thresholds_denormalized_multiplier (const WorkThresholdsDto * dto,
double multiplier,
uint64_t threshold);

uint64_t rsn_work_thresholds_difficulty (const WorkThresholdsDto * dto,
uint8_t work_version,
const uint8_t (*root)[32],
uint64_t work);

double rsn_work_thresholds_normalized_multiplier (const WorkThresholdsDto * dto,
double multiplier,
uint64_t threshold);

void rsn_work_thresholds_publish_beta (WorkThresholdsDto * dto);

void rsn_work_thresholds_publish_dev (WorkThresholdsDto * dto);

void rsn_work_thresholds_publish_full (WorkThresholdsDto * dto);

void rsn_work_thresholds_publish_test (WorkThresholdsDto * dto);

uint64_t rsn_work_thresholds_threshold (const WorkThresholdsDto * dto,
const BlockDetailsDto * details);

uint64_t rsn_work_thresholds_threshold2 (const WorkThresholdsDto * dto,
uint8_t work_version,
const BlockDetailsDto * details);

uint64_t rsn_work_thresholds_threshold_base (const WorkThresholdsDto * dto, uint8_t work_version);

uint64_t rsn_work_thresholds_threshold_entry (const WorkThresholdsDto * dto,
uint8_t work_version,
uint8_t block_type);

bool rsn_work_thresholds_validate_entry (const WorkThresholdsDto * dto,
uint8_t work_version,
const uint8_t (*root)[32],
uint64_t work);

uint64_t rsn_work_thresholds_value (const WorkThresholdsDto * dto,
const uint8_t (*root)[32],
uint64_t work);

int32_t rsn_working_path (uint16_t network, uint8_t * result, uintptr_t size);

} // extern "C"

} // namespace rsnano

#endif // rs_nano_bindings_hpp
