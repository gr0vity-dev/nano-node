use crate::cli::GlobalArgs;
use clap::Parser;
use rsnano_daemon::DaemonBuilder;
use rsnano_node::config::NodeFlags;
use rsnano_nullable_tracing_subscriber::TracingInitializer;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct RunDaemonArgs {
    /// Turn off automatic wallet backup process
    #[arg(long)]
    disable_backup: bool,
    /// Turn off the ability for ongoing bootstraps to occur
    #[arg(long)]
    disable_ongoing_bootstrap: bool,
    /// Turn off inbound confirm_req aggregation and its worker threads
    #[arg(long)]
    disable_request_aggregator: bool,
    /// Turn off outgoing confirm_req solicitation from the AEC ticker
    #[arg(long)]
    disable_confirm_req: bool,
    /// Turn off the hinted scheduler
    #[arg(long)]
    disable_hinted_scheduler: bool,
    /// Turn off the optimistic scheduler
    #[arg(long)]
    disable_optimistic_scheduler: bool,
    /// Turn off the manual scheduler
    #[arg(long)]
    disable_manual_scheduler: bool,
    /// Turn off the request loop
    #[arg(long)]
    disable_request_loop: bool,
    /// Turn off the rep crawler process
    #[arg(long)]
    disable_rep_crawler: bool,
    /// Do not provide any telemetry data to nodes requesting it. Responses are still made to requests, but they will have an empty payload.
    #[arg(long)]
    disable_providing_telemetry_metrics: bool,
    /// Disables block republishing by disabling the local_block_broadcaster component
    #[arg(long)]
    disable_block_processor_republishing: bool,
    /// Allow multiple connections to the same peer in bootstrap attempts
    #[arg(long)]
    allow_bootstrap_peers_duplicates: bool,
    /// Enable voting
    #[arg(long)]
    enable_voting: bool,
    /// Increase bootstrap processor limits to allow more blocks before hitting full state and verify/write more per database call. Also disable deletion of processed unchecked blocks.
    #[arg(long)]
    fast_bootstrap: bool,
    /// Increase batch signature verification size in block processor, default 0 (limited by config signature_checker_threads), unlimited for fast_bootstrap
    #[arg(long)]
    block_processor_verification_size: Option<usize>,
    /// Skip ledger consistency check on startup, this is not recommended and should only be used for testing or recovery purposes
    #[arg(long)]
    skip_consistency_check: bool,
}

impl RunDaemonArgs {
    pub(crate) fn run_daemon(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        TracingInitializer::default().init();
        let network = global_args.network;
        let flags = self.get_flags();
        DaemonBuilder::new(network)
            .flags(flags)
            .data_path(&global_args.data_path)
            .run(shutdown_signal())
    }

    pub(crate) fn get_flags(&self) -> NodeFlags {
        let mut flags = NodeFlags::new();
        flags.disable_backup = self.disable_backup;
        flags.disable_ongoing_bootstrap = self.disable_ongoing_bootstrap;
        flags.disable_request_aggregator = self.disable_request_aggregator;
        flags.disable_confirm_req = self.disable_confirm_req;
        flags.disable_hinted_scheduler = self.disable_hinted_scheduler;
        flags.disable_optimistic_scheduler = self.disable_optimistic_scheduler;
        flags.disable_manual_scheduler = self.disable_manual_scheduler;
        flags.disable_rep_crawler = self.disable_rep_crawler;
        flags.disable_request_loop = self.disable_request_loop;
        flags.disable_providing_telemetry_metrics = self.disable_providing_telemetry_metrics;
        flags.disable_block_processor_republishing = self.disable_block_processor_republishing;
        flags.allow_bootstrap_peers_duplicates = self.allow_bootstrap_peers_duplicates;
        flags.enable_voting = self.enable_voting;
        flags.fast_bootstrap = self.fast_bootstrap;
        flags.skip_consistency_check = self.skip_consistency_check;
        flags
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
