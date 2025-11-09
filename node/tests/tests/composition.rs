use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use rsnano_node::{Node, NodeBuilder};
use rsnano_types::Networks;

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

    let network_services = node.network_services();
    let network_read = network_services.network.read().unwrap();
    assert!(
        !network_read.is_stopped(),
        "null node should start the network service"
    );

    assert!(
        node.wallet_services().work_factory.work_threads() > 0,
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
