use anyhow::anyhow;
use clap::{ArgGroup, Parser};

use rsnano_ledger::LedgerConstants;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;
use rsnano_store_lmdb::{
    LmdbConfirmationHeightStore, LmdbLedgerWriteTxn, default_ledger_lmdb_options,
};
use rsnano_types::{Account, ConfirmationHeightInfo, Networks};

use crate::cli::GlobalArgs;

#[derive(Parser, PartialEq, Debug)]
#[command(group = ArgGroup::new("input1")
    .args(&["account", "all"])
    .required(true))]
pub(crate) struct ConfirmationHeightArgs {
    /// Clears the confirmation height of the account
    #[arg(long, group = "input1")]
    account: Option<String>,
    /// Clears the confirmation height of all accounts
    #[arg(long, group = "input1")]
    all: bool,
}

impl ConfirmationHeightArgs {
    pub(crate) fn confirmation_height(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let path = global_args.data_path.join("data.ldb");

        let genesis_block = match global_args.network {
            Networks::NanoDevNetwork => LedgerConstants::dev().genesis_block,
            Networks::NanoBetaNetwork => LedgerConstants::beta().genesis_block,
            Networks::NanoLiveNetwork => LedgerConstants::live().genesis_block,
            Networks::NanoTestNetwork => LedgerConstants::test().genesis_block,
            Networks::Invalid => unreachable!(),
        };

        let genesis_account = genesis_block.account();
        let genesis_hash = genesis_block.hash();

        let env_factory = LmdbEnvironmentFactory::default();
        let options = default_ledger_lmdb_options(path);
        let env = env_factory.create(options)?;
        let confirmation_height_store = LmdbConfirmationHeightStore::new(&env)?;

        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        if let Some(account_hex) = &self.account {
            let account = Account::parse(account_hex).ok_or_else(|| anyhow!("Invalid account"))?;
            let mut conf_height_reset_num = 0;

            let mut info = confirmation_height_store
                .get(&txn, &account)
                .expect("Could not find account");

            if account == genesis_account {
                conf_height_reset_num += 1;
                info.height = conf_height_reset_num;
                info.frontier = genesis_hash;
                confirmation_height_store.put(&mut txn, &account, &info);
            } else {
                confirmation_height_store.del(&mut txn, &account);
            }
            println!(
                "Confirmation height of account {:?} is set to {:?}",
                account_hex, conf_height_reset_num
            );
        } else {
            confirmation_height_store.clear(&mut txn);
            confirmation_height_store.put(
                &mut txn,
                &genesis_account,
                &ConfirmationHeightInfo::new(1, genesis_hash),
            );
            println!(
                "Confirmation heights of all accounts (except genesis which is set to 1) are set to 0"
            );
        }
        txn.commit()
            .map_err(|e| anyhow::anyhow!(format!("commit failed: {e}")))?;

        Ok(())
    }
}
