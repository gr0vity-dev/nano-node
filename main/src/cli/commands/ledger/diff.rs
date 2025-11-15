use anyhow::anyhow;
use clap::Parser;
use rsnano_nullable_console::Console;
use rsnano_nullable_fs::NullableFilesystem;
use rsnano_nullable_lmdb::{LmdbEnvironment, LmdbEnvironmentFactory};
use rsnano_store_lmdb::{EnvironmentFlags, EnvironmentOptions, LmdbLedgerReadTxn, LmdbStore};
use rsnano_types::{Account, AccountInfo, BlockHash};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Parser, PartialEq, Eq, Debug)]
pub(crate) struct LedgerDiffArgs {
    left: PathBuf,
    right: PathBuf,
}

#[derive(Default)]
pub(crate) struct LedgerDiff {
    console: Console,
    fs: NullableFilesystem,
    lmdb_env_factory: LmdbEnvironmentFactory,
}

impl LedgerDiff {
    pub(crate) fn run(&self, args: LedgerDiffArgs) -> anyhow::Result<()> {
        let env_left = self.open_ledger(args.left)?;
        let env_right = self.open_ledger(args.right)?;
        let store_left = LmdbStore::new(env_left)?;
        let store_right = LmdbStore::new(env_right)?;
        let txn_l = LmdbLedgerReadTxn::new(store_left.env.begin_read());
        let txn_r = LmdbLedgerReadTxn::new(store_right.env.begin_read());
        let mut acc_iter_l = store_left.account.iter(&txn_l).fuse();
        let mut acc_iter_r = store_right.account.iter(&txn_r).fuse();
        let mut current_l = acc_iter_l.next();
        let mut current_r = acc_iter_r.next();

        let diff = |account: &Account,
                    info_l: Option<&AccountInfo>,
                    info_r: Option<&AccountInfo>| match (info_l, info_r) {
            (None, None) => unreachable!(),
            (None, Some(r)) => Some(DiffEntry {
                account: *account,
                left: None,
                right: Some(AccountData {
                    unconf_frontier: r.head,
                    conf_frontier: store_right
                        .confirmation_height
                        .get(&txn_r, account)
                        .unwrap_or_default()
                        .frontier,
                    height: r.block_count,
                }),
            }),
            (Some(l), None) => Some(DiffEntry {
                account: *account,
                left: Some(AccountData {
                    unconf_frontier: l.head,
                    conf_frontier: store_left
                        .confirmation_height
                        .get(&txn_l, account)
                        .unwrap_or_default()
                        .frontier,
                    height: l.block_count,
                }),
                right: None,
            }),
            (Some(l), Some(r)) => {
                let conf_frontier_l = store_left
                    .confirmation_height
                    .get(&txn_l, account)
                    .unwrap_or_default()
                    .frontier;

                let conf_frontier_r = store_right
                    .confirmation_height
                    .get(&txn_r, account)
                    .unwrap_or_default()
                    .frontier;

                if l.head == r.head && conf_frontier_l == conf_frontier_r {
                    None
                } else {
                    Some(DiffEntry {
                        account: *account,
                        left: Some(AccountData {
                            unconf_frontier: l.head,
                            conf_frontier: conf_frontier_l,
                            height: l.block_count,
                        }),
                        right: Some(AccountData {
                            unconf_frontier: r.head,
                            conf_frontier: conf_frontier_r,
                            height: r.block_count,
                        }),
                    })
                }
            }
        };

        loop {
            let diff_entry: Option<DiffEntry>;

            match (&current_l, &current_r) {
                (None, None) => break,
                (None, Some((acc_r, info_r))) => {
                    diff_entry = diff(acc_r, None, Some(info_r));
                    current_r = acc_iter_r.next();
                }
                (Some((acc_l, info_l)), None) => {
                    diff_entry = diff(acc_l, Some(info_l), None);
                    current_l = acc_iter_l.next();
                }
                (Some((acc_l, info_l)), Some((acc_r, info_r))) => {
                    if acc_l < acc_r {
                        diff_entry = diff(acc_l, Some(info_l), None);
                        current_l = acc_iter_l.next();
                    } else if acc_l > acc_r {
                        diff_entry = diff(acc_r, None, Some(info_r));
                        current_r = acc_iter_r.next();
                    } else {
                        diff_entry = diff(acc_l, Some(info_l), Some(info_r));
                        current_l = acc_iter_l.next();
                        current_r = acc_iter_r.next();
                    }
                }
            };

            if let Some(entry) = diff_entry {
                self.console.println(serde_json::to_string(&entry).unwrap());
            }
        }
        Ok(())
    }

    fn open_ledger(&self, path: impl Into<PathBuf>) -> anyhow::Result<LmdbEnvironment> {
        let path = path.into();
        self.ensure_ledger_file_exists(&path)?;
        let env = self.lmdb_env_factory.create(EnvironmentOptions {
            max_dbs: 128,
            map_size: 256 * 1024 * 1024 * 1024,
            flags: EnvironmentFlags::NO_SUB_DIR,
            path,
        })?;
        Ok(env)
    }

    fn ensure_ledger_file_exists(&self, path: &Path) -> anyhow::Result<()> {
        if self.fs.exists(path) {
            Ok(())
        } else {
            Err(anyhow!("Ledger file not found: {:?}", path))
        }
    }
}

#[derive(Serialize)]
pub(crate) struct DiffEntry {
    account: Account,
    left: Option<AccountData>,
    right: Option<AccountData>,
}

#[derive(Serialize)]
pub(crate) struct AccountData {
    unconf_frontier: BlockHash,
    conf_frontier: BlockHash,
    height: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_error_when_first_ledger_file_not_found() {
        let console = Console::new_null();
        let fs = NullableFilesystem::new_null();
        let lmdb_env_factory = LmdbEnvironmentFactory::new_null();
        let app = LedgerDiff {
            console,
            fs,
            lmdb_env_factory,
        };
        let args = LedgerDiffArgs {
            left: "left.ldb".into(),
            right: "right.ldb".into(),
        };
        let err = app.run(args).unwrap_err();
        assert_eq!(err.to_string(), "Ledger file not found: \"left.ldb\"");
    }

    #[test]
    fn shows_error_when_second_ledger_file_not_found() {
        let console = Console::new_null();
        let fs = NullableFilesystem::null_builder()
            .path_exists("left.ldb")
            .finish();
        let lmdb_env_factory = LmdbEnvironmentFactory::new_null();

        let app = LedgerDiff {
            console,
            fs,
            lmdb_env_factory,
        };
        let args = LedgerDiffArgs {
            left: "left.ldb".into(),
            right: "right.ldb".into(),
        };
        let err = app.run(args).unwrap_err();
        assert_eq!(err.to_string(), "Ledger file not found: \"right.ldb\"");
    }

    #[test]
    fn print_nothing_when_ledgers_are_equal() {
        let console = Console::new_null();
        let stdout_tracker = console.track();
        let lmdb_env_factory = LmdbEnvironmentFactory::new_null();
        let env_tracker = lmdb_env_factory.track();

        let fs = NullableFilesystem::null_builder()
            .path_exists("left.ldb")
            .path_exists("right.ldb")
            .finish();

        let app = LedgerDiff {
            console,
            fs,
            lmdb_env_factory,
        };
        let args = LedgerDiffArgs {
            left: "left.ldb".into(),
            right: "right.ldb".into(),
        };

        app.run(args).unwrap();

        let created_envs = env_tracker.output();
        assert_eq!(
            created_envs,
            vec![
                EnvironmentOptions {
                    max_dbs: 128,
                    flags: EnvironmentFlags::NO_SUB_DIR,
                    path: "left.ldb".into(),
                    map_size: 256 * 1024 * 1024 * 1024,
                },
                EnvironmentOptions {
                    max_dbs: 128,
                    flags: EnvironmentFlags::NO_SUB_DIR,
                    path: "right.ldb".into(),
                    map_size: 256 * 1024 * 1024 * 1024,
                },
            ]
        );
        assert_eq!(stdout_tracker.output(), Vec::<String>::new());
    }
}
