use anyhow::{Context, anyhow};
use clap::{CommandFactory, Parser, Subcommand};
use commands::{
    config::ConfigCommand,
    ledger::{LedgerCommand, run_ledger_command},
    node::NodeCommand,
    utils::{UtilsCommand, run_utils_command},
    wallets::{WalletsCommand, run_wallets_command},
};
use rsnano_node::{Node, NodeBuilder, working_path_for};
use rsnano_nullable_console::Console;
use rsnano_types::{Networks, PrivateKeyFactory};
use std::{path::PathBuf, str::FromStr};
use store_traits::config::{LedgerBackend, LmdbConfig, RocksDbConfig};

mod commands;

#[derive(Parser, PartialEq, Debug, Default)]
pub(crate) struct CommandLineArgs {
    /// Uses the supplied network (live, test, beta or dev)
    #[arg(long)]
    network: Option<String>,

    /// Uses the supplied path as the data directory
    #[arg(long)]
    data_path: Option<String>,

    /// Storage backend to use (lmdb | rocksdb)
    #[arg(long)]
    storage_backend: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, PartialEq, Debug)]
pub(crate) enum Commands {
    /// Commands related to configs
    Config(ConfigCommand),
    /// Commands related to the ledger
    Ledger(LedgerCommand),
    /// Commands related to running the node
    Node(NodeCommand),
    /// Utils related to keys and accounts
    Utils(UtilsCommand),
    /// Commands to manage wallets
    Wallets(WalletsCommand),
}

pub(crate) struct Cli {}

impl Cli {
    pub(crate) fn run(
        &self,
        infra: &mut CliInfrastructure,
        args: CommandLineArgs,
    ) -> anyhow::Result<()> {
        let global_args = self.get_global_args(&args)?;

        match args.command {
            Some(Commands::Wallets(command)) => run_wallets_command(global_args, command)?,
            Some(Commands::Utils(command)) => run_utils_command(infra, command)?,
            Some(Commands::Node(command)) => command.run(global_args)?,
            Some(Commands::Ledger(command)) => run_ledger_command(global_args, command)?,
            Some(Commands::Config(command)) => command.run(global_args)?,
            None => CommandLineArgs::command().print_long_help()?,
        }
        Ok(())
    }

    fn get_global_args(&self, args: &CommandLineArgs) -> anyhow::Result<GlobalArgs> {
        let network = self.get_network(args)?;
        let data_path = self.get_data_path(args)?;
        let storage_backend = self.get_storage_backend(args)?;
        Ok(GlobalArgs {
            network,
            data_path,
            storage_backend,
        })
    }

    fn get_network(&self, args: &CommandLineArgs) -> anyhow::Result<Networks> {
        args.network
            .as_ref()
            .map(|str| Networks::from_str(str).map_err(|e| anyhow!(e)))
            .transpose()
            .map(|net| net.unwrap_or(Networks::NanoLiveNetwork))
    }

    fn get_storage_backend(&self, args: &CommandLineArgs) -> anyhow::Result<Option<LedgerBackend>> {
        match args.storage_backend.as_deref() {
            None => Ok(None),
            Some("lmdb") => Ok(Some(LedgerBackend::Lmdb(LmdbConfig::default()))),
            Some("rocksdb") => Ok(Some(LedgerBackend::RocksDb(RocksDbConfig::default()))),
            Some(other) => Err(anyhow!("Unsupported storage backend '{other}'")),
        }
    }

    fn get_data_path(&self, args: &CommandLineArgs) -> anyhow::Result<PathBuf> {
        if let Some(path) = &args.data_path {
            return PathBuf::from_str(path).context("Not a valid data path");
        }
        working_path_for(self.get_network(args)?).ok_or_else(|| anyhow!("No data path found"))
    }
}

pub(crate) struct GlobalArgs {
    pub network: Networks,
    pub data_path: PathBuf,
    pub storage_backend: Option<LedgerBackend>,
}

pub(crate) fn build_node(args: &GlobalArgs) -> anyhow::Result<Node> {
    let builder = NodeBuilder::new(args.network).data_path(&args.data_path);
    let builder = if let Some(backend) = &args.storage_backend {
        builder.storage_backend(backend.clone())
    } else {
        builder
    };
    builder.finish()
}

#[derive(Default)]
pub(crate) struct CliInfrastructure {
    pub key_factory: PrivateKeyFactory,
    pub console: Console,
}

impl CliInfrastructure {
    #[allow(dead_code)]
    pub fn new_null() -> Self {
        Self {
            key_factory: PrivateKeyFactory::new_null(),
            console: Console::new_null(),
        }
    }
}
