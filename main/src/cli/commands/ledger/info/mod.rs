use clap::{CommandFactory, Parser, Subcommand};

use rsnano_store_lmdb::{LmdbLedgerReadTxn, LmdbPeerStore, default_ledger_lmdb_options};

use crate::cli::GlobalArgs;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;

#[derive(Subcommand, PartialEq, Debug)]
pub(crate) enum InfoSubcommands {
    /// Displays peer IPv6:port connections
    Peers,
}

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct InfoCommand {
    #[command(subcommand)]
    pub subcommand: Option<InfoSubcommands>,
}

impl InfoCommand {
    pub(crate) fn run(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        match &self.subcommand {
            Some(InfoSubcommands::Peers) => self.peers(global_args)?,
            None => InfoCommand::command().print_long_help()?,
        }

        Ok(())
    }

    fn peers(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let path = global_args.data_path.join("data.ldb");
        let options = default_ledger_lmdb_options(path);
        let env = LmdbEnvironmentFactory::default().create(options)?;
        let peer_store = LmdbPeerStore::new(&env)?;
        let txn = LmdbLedgerReadTxn::new(env.begin_read());

        for peer in peer_store.iter(&txn) {
            println!("{:?}", peer.0);
        }

        txn.commit();
        Ok(())
    }
}
