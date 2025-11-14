mod confirmation_height;
mod final_vote;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use rsnano_store_lmdb::{
    LmdbLedgerWriteTxn, LmdbOnlineWeightStore, LmdbPeerStore, default_ledger_lmdb_options,
};

use crate::cli::GlobalArgs;
use confirmation_height::ConfirmationHeightArgs;
use final_vote::FinalVoteArgs;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct ClearCommand {
    #[command(subcommand)]
    pub subcommand: Option<ClearSubcommands>,
}

#[derive(Subcommand, PartialEq, Debug)]
pub(crate) enum ClearSubcommands {
    /// Clears final votes
    FinalVote(FinalVoteArgs),
    /// Clears online weight history records
    OnlineWeight,
    /// Clears online peers database
    Peers,
    /// Clears the confirmation height of accounts
    ConfirmationHeight(ConfirmationHeightArgs),
}

impl ClearCommand {
    pub(crate) fn run(&self, global_args: GlobalArgs) -> Result<()> {
        match &self.subcommand {
            Some(ClearSubcommands::FinalVote(args)) => args.final_vote(global_args)?,
            Some(ClearSubcommands::ConfirmationHeight(args)) => {
                args.confirmation_height(global_args)?
            }
            Some(ClearSubcommands::OnlineWeight) => self.online_weight(global_args)?,
            Some(ClearSubcommands::Peers) => self.peers(global_args)?,
            None => ClearCommand::command().print_long_help()?,
        }

        Ok(())
    }

    fn online_weight(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let path = global_args.data_path.join("data.ldb");
        let options = default_ledger_lmdb_options(path);
        let env = LmdbEnvironmentFactory::default().create(options)?;
        let online_weight_store = LmdbOnlineWeightStore::new(&env)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        online_weight_store.clear(&mut txn);
        txn.commit();

        println!("Online weight records were cleared from the database");
        Ok(())
    }

    fn peers(&self, global_args: GlobalArgs) -> Result<()> {
        let path = global_args.data_path.join("data.ldb");
        let options = default_ledger_lmdb_options(path);
        let env = LmdbEnvironmentFactory::default().create(options)?;
        let peer_store = LmdbPeerStore::new(&env)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        peer_store.clear(&mut txn);
        txn.commit();

        println!("Peers were cleared from the database");
        Ok(())
    }
}
