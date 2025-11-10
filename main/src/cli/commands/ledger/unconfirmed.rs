use crate::cli::GlobalArgs;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;
use rsnano_store_lmdb::{EnvironmentFlags, EnvironmentOptions, LmdbStore};

pub(crate) fn print_unconfirmed_accounts(args: GlobalArgs) -> anyhow::Result<()> {
    let env_factory = LmdbEnvironmentFactory::default();
    let env = env_factory.create(EnvironmentOptions {
        max_dbs: 128,
        map_size: 256 * 1024 * 1024 * 1024,
        flags: EnvironmentFlags::NO_SUB_DIR,
        path: args.data_path.join("data.ldb"),
    })?;
    let store = LmdbStore::new(env)?;
    let txn = store.begin_read();
    for (account, info) in store.account().iter(&txn) {
        let conf_height = store
            .confirmation_height
            .get(&txn, &account)
            .unwrap_or_default()
            .height;
        if conf_height != info.block_count {
            println!("{}", account.encode_account());
        }
    }
    Ok(())
}
