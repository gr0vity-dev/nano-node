use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use rsnano_node::{Node, NodeBuildError, NodeBuilder, config::get_node_toml_config_path};
use rsnano_types::Networks;
use store_traits::config::{LedgerBackend, RocksDbConfig};
use test_helpers::System;

fn unique_path(suffix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rsnano-compose-{suffix}-{}-{}",
        std::process::id(),
        nanos
    ))
}

#[test]
fn compose_null_node_exposes_basic_services() {
    let node = Node::new_null();

    let network = node.network_subsystem();
    assert!(
        !network.is_stopped(),
        "null node should start the network service"
    );

    assert!(
        node.wallet_services().work_threads() > 0,
        "work factory must provision at least one thread"
    );
}

#[test]
fn composition_fails_when_data_path_is_a_file() {
    let temp_dir = unique_path("base");
    fs::create_dir_all(&temp_dir).unwrap();
    let data_file = temp_dir.join("not_a_directory");
    fs::write(&data_file, b"not a directory").unwrap();

    let builder = NodeBuilder::new(Networks::NanoDevNetwork).data_path(&data_file);

    let err = builder.finish().err().expect("composition should fail");
    let message = err.to_string();
    assert!(
        message.contains("data dir") || message.contains("node ID key file"),
        "expected data directory related error, got: {message}"
    );

    fs::remove_file(&data_file).ok();
    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn builder_fails_when_genesis_missing_from_ledger() {
    let temp_dir = unique_path("missing-genesis");
    fs::create_dir_all(&temp_dir).unwrap();

    let mut node = NodeBuilder::new(Networks::NanoDevNetwork)
        .data_path(&temp_dir)
        .finish()
        .expect("node builds for setup");

    let genesis_hash = node.network_params().ledger.genesis_block.hash();
    let ledger = node.ledger_query_services().ledger_arc();
    let mut txn = ledger.store.begin_write();
    ledger.store.block().del(txn.as_mut(), &genesis_hash);
    txn.commit().unwrap();
    node.stop();
    drop(node);

    let err = NodeBuilder::new(Networks::NanoDevNetwork)
        .data_path(&temp_dir)
        .finish()
        .err()
        .expect("builder should fail when genesis block is missing");
    assert!(matches!(err, NodeBuildError::GenesisBlockMissing { .. }));

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn node_builder_supports_rocksdb_backend_via_config() {
    let mut system = System::new();
    let mut config = System::default_config();
    config.ledger_store_config.backend = LedgerBackend::RocksDb(RocksDbConfig::default());
    let node = system.build_node().config(config).finish();

    let ledger_dir = node.data_path().join("data.rocksdb");
    assert!(
        ledger_dir.join("CURRENT").exists(),
        "RocksDB ledger should create CURRENT file at {:?}",
        ledger_dir
    );
}

#[test]
fn node_builder_loads_backend_from_toml_file() {
    let temp_dir = unique_path("rocksdb-config-file");
    fs::create_dir_all(&temp_dir).unwrap();
    let config_path = get_node_toml_config_path(&temp_dir);
    let toml = r#"[node.storage]
backend = "rocksdb"

[node.storage.rocksdb]
max_open_files = 64
"#;
    fs::write(&config_path, toml).unwrap();

    let mut node = NodeBuilder::new(Networks::NanoDevNetwork)
        .data_path(&temp_dir)
        .finish()
        .expect("node loads from config");
    assert!(matches!(
        node.config().ledger_store_config.backend,
        LedgerBackend::RocksDb(_)
    ));
    node.stop();
    fs::remove_dir_all(&temp_dir).ok();
}
