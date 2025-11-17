use num_derive::FromPrimitive;
use serde::Serialize;
use serde_variant::to_variant_name;

/// Primary statistics type
#[repr(u8)]
#[derive(FromPrimitive, Serialize, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum StatType {
    Error,
    Message,
    Ledger,
    LedgerIterator,
    Rollback,
    Network,
    VoteProcessor,
    VoteProcessorTier,
    VoteProcessorOverfill,
    VoteRebroadcaster,
    Election,
    HttpCallbacks,
    TcpServer,
    TcpChannels,
    ConfirmationHeight,
    ConfirmationObserver,
    ConfirmingSet,
    Drop,
    Aggregator,
    Requests,
    RequestAggregator,
    RequestAggregatorReplies,
    Filter,
    Telemetry,
    VoteGenerator,
    VoteGeneratorFinal,
    VoteCache,
    VoteCacheProcessor,
    Hinting,
    BlockProcessor,
    Bootstrap,
    BootstrapVerifyBlocks,
    BootstrapVerifyFrontiers,
    BootstrapProcess,
    BootstrapRequest,
    BootstrapReply,
    BootstrapNext,
    BootstrapFrontiers,
    BootstrapAccountSets,
    BootstrapFrontierScan,
    BootstrapTimeout,
    BootstrapServer,
    BootstrapServerRequest,
    BootstrapServerOverfill,
    BootstrapServerResponse,
    ActiveElections,
    ActiveElectionsConfirmed,
    ActiveElectionsDropped,
    ActiveElectionsTimeout,
    ActiveElectionsCancelled,
    BoundedBacklog,
    ElectionScheduler,
    OptimisticScheduler,
    RepCrawler,
    LocalBlockBroadcaster,
    RepTiers,
    PeerHistory,
}

impl StatType {
    pub fn as_str(&self) -> &'static str {
        to_variant_name(self).unwrap_or_default()
    }
}

// Optional detail type
#[repr(u16)]
#[derive(FromPrimitive, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DetailType {
    // common
    All = 0,
    Ok,
    Loop,
    LoopCleanup,
    Process,
    Processed,
    Ignored,
    Update,
    Updated,
    Inserted,
    Request,
    RequestFailed,
    Broadcast,
    Cleanup,
    Top,
    None,
    Unknown,
    Cache,
    Rebroadcast,
    Triggered,
    Duplicate,
    Confirmed,
    Cemented,
    Cooldown,
    Recovered,
    Prioritized,
    Pending,
    Requeued,
    Evicted,

    // processing queue
    Queue,
    Overfill,
    Batch,

    // error specific
    InsufficientWork,
    HttpCallback,
    InvalidNetwork,
    Conflict,

    // confirmation_observer specific
    InactiveConfHeight,

    // ledger, block, bootstrap
    Send,
    Receive,
    Open,
    Change,
    StateBlock,
    EpochBlock,
    Fork,
    Old,
    GapPrevious,
    GapSource,
    Rollback,
    RollbackFailed,
    Progress,
    BadSignature,
    NegativeSpend,
    Unreceivable,
    GapEpochOpenPending,
    OpenedBurnAccount,
    BalanceMismatch,
    RepresentativeMismatch,
    BlockPosition,
    RocksDbBlockIndex,
    RocksDbBlockData,
    RocksDbAccounts,
    RocksDbPending,
    RocksDbConfirmationHeight,
    RocksDbRepWeights,
    RocksDbSuccessors,
    RocksDbOnlineWeight,
    RocksDbPruned,
    RocksDbFinalVotes,
    RocksDbPeers,
    RocksDbVersion,
    RocksDbOther,

    // block source
    Live,
    LiveOriginator,
    Bootstrap,
    BootstrapLegacy,
    Unchecked,
    Local,
    Forced,
    Election,

    // message specific
    NotAType,
    Invalid,
    Keepalive,
    Publish,
    ConfirmReq,
    ConfirmAck,
    NodeIdHandshake,
    TelemetryReq,
    TelemetryAck,
    AscPullReq,
    AscPullAck,
    #[cfg(feature = "ledger_snapshots")]
    Preproposal,
    #[cfg(feature = "ledger_snapshots")]
    Proposal,
    #[cfg(feature = "ledger_snapshots")]
    ProposalVote,

    // dropped messages
    ConfirmAckZeroAccount,

    // bootstrap, callback
    Initiate,

    // bootstrap specific
    BulkPull,
    BulkPullAccount,
    BulkPush,
    FrontierReq,

    // vote result
    Vote,

    // election specific
    GenerateVoteNormal,
    GenerateVoteFinal,
    ConfirmationRequest,

    // election types
    Manual,
    Priority,
    Hinted,
    Optimistic,

    // received messages
    InvalidHeader,
    InvalidMessageType,
    InvalidKeepaliveMessage,
    InvalidPublishMessage,
    InvalidConfirmReqMessage,
    InvalidConfirmAckMessage,
    InvalidNodeIdHandshakeMessage,
    InvalidTelemetryReqMessage,
    InvalidTelemetryAckMessage,
    InvalidBulkPullMessage,
    InvalidBulkPullAccountMessage,
    InvalidFrontierReqMessage,
    InvalidAscPullReqMessage,
    InvalidAscPullAckMessage,
    MessageSizeTooBig,
    OutdatedVersion,

    // network
    LoopKeepalive,
    LoopReachoutCached,
    ReachoutLive,
    ReachoutCached,

    // traffic
    Generic,
    BootstrapServer,
    BootstrapRequests,
    BlockBroadcast,
    BlockBroadcastRpc,
    BlockBroadcastInitial,
    ConfirmationRequests,
    VoteRebroadcast,
    RepCrawler,
    VoteReply,
    Telemetry,

    // tcp_channels
    ChannelAccepted,
    Outdated,

    // tcp_server
    HandshakeAbort,
    HandshakeError,

    // confirmation height
    BlocksConfirmed,

    // request aggregator
    AggregatorAccepted,
    AggregatorDropped,

    // requests
    RequestsCachedHashes,
    RequestsGeneratedHashes,
    RequestsCachedVotes,
    RequestsGeneratedVotes,
    RequestsCannotVote,
    RequestsUnknown,
    RequestsNonFinal,
    RequestsFinal,

    // request_aggregator
    RequestHashes,
    OverfillHashes,
    NormalVote,
    FinalVote,

    // duplicate
    DuplicatePublishMessage,
    DuplicateConfirmAckMessage,

    // telemetry
    InvalidSignature,
    NodeIdMismatch,
    GenesisMismatch,
    EmptyPayload,
    CleanupOutdated,

    // vote generator
    GeneratorBroadcasts,
    GeneratorReplies,
    GeneratorRepliesDiscarded,
    GeneratorSpacing,
    SentPr,
    SentNonPr,

    // hinting
    MissingBlock,
    DependentUnconfirmed,
    AlreadyConfirmed,
    Activate,
    ActivateImmediate,

    // bootstrap server
    Response,
    Blocks,
    ChannelFull,
    Frontiers,
    AccountInfo,

    // backlog
    Activated,
    ActivateFailed,
    ActivateSkip,
    ActivateFull,
    Scanned,

    // active
    Insert,
    InsertFailed,
    ActivateImmediately,

    // election scheduler
    InsertManual,
    EraseOldest,

    // bootstrap
    MissingTag,
    Reply,
    Timeout,
    AccountInfoEmpty,
    LoopFrontiers,
    InvalidResponseType,
    InvalidResponse,

    Prioritize,
    PrioritizeFailed,
    Block,
    BlockFailed,
    Unblock,

    NextNone,
    NextFrontier,

    PriorityInsert,
    PriorityErase,
    PriorityUnblocked,
    PriorityEraseThreshold,
    PriorityEraseBlock,
    SyncDependencies,
    BlockingDecayed,
    DependencySynced,

    // rep_crawler
    QuerySent,
    QueryDuplicate,
    QueryTimeout,
    QueryCompletion,
    CrawlAggressive,
    CrawlNormal,

    // rep tiers
    Tier1,
    Tier2,
    Tier3,

    // confirming_set
    NotifyIntermediate,
    AlreadyCemented,
    Cementing,
    CementedHash,
    CementingFailed,

    // election_state
    Passive,
    Active,
    ExpiredConfirmed,
    ExpiredUnconfirmed,
    Cancelled,

    // election_status_type
    ActiveConfirmedQuorum,
    ActiveConfirmationHeight,
    InactiveConfirmationHeight,

    // query_type
    BlocksByHash,
    BlocksByAccount,
    AccountInfoByHash,

    // bounded backlog
    GatheredTargets,
    PerformingRollbacks,
    NoTargets,
    RollbackMissingBlock,
    RollbackSkipped,
    LoopScan,
}

impl DetailType {
    pub fn as_str(&self) -> &'static str {
        to_variant_name(self).unwrap_or_default()
    }
}

/// Direction of the stat. If the direction is irrelevant, use In
#[derive(FromPrimitive, PartialEq, PartialOrd, Eq, Ord, Clone, Copy, Debug, Hash)]
#[repr(u8)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

#[repr(u8)]
#[derive(FromPrimitive, Serialize, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Sample {
    ActiveElectionDuration,
    BootstrapTagDuration,
    RepResponseTime,
    VoteGeneratorFinalHashes,
    VoteGeneratorHashes,
}

impl Sample {
    pub fn as_str(&self) -> &'static str {
        to_variant_name(self).unwrap_or_default()
    }
}
