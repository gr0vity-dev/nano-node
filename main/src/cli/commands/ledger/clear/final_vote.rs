use anyhow::anyhow;
use clap::{ArgGroup, Parser};

use rsnano_store_lmdb::{LmdbFinalVoteStore, LmdbLedgerWriteTxn, default_ledger_lmdb_options};
use rsnano_types::QualifiedRoot;

use crate::cli::GlobalArgs;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;

#[derive(Parser, PartialEq, Debug)]
#[command(group = ArgGroup::new("input1")
    .args(&["root", "all"])
    .required(true))]
pub(crate) struct FinalVoteArgs {
    /// Clears the supplied final vote
    #[arg(long, group = "input1")]
    root: Option<String>,
    /// Clears all final votes (not recommended)
    #[arg(long, group = "input1")]
    all: bool,
}

impl FinalVoteArgs {
    pub(crate) fn final_vote(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let path = global_args.data_path.join("data.ldb");
        let options = default_ledger_lmdb_options(path);
        let env = LmdbEnvironmentFactory::default().create(options)?;
        let final_vote_store = LmdbFinalVoteStore::new(&env)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        if let Some(root) = &self.root {
            let root_decoded =
                QualifiedRoot::decode_hex(root).ok_or_else(|| anyhow!("Invalid root"))?;
            final_vote_store.del(&mut txn, &root_decoded);
            println!("Successfully cleared final vote");
        } else {
            final_vote_store.clear(&mut txn);
            println!("All final votes were cleared from the database");
        }
        txn.commit()
            .map_err(|e| anyhow::anyhow!(format!("commit failed: {e}")))?;

        Ok(())
    }
}
