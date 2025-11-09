use rsnano_ledger::{DEV_GENESIS_ACCOUNT, test_helpers::UnsavedBlockLatticeBuilder};
use rsnano_types::{Amount, DEV_GENESIS_KEY, WalletId};
use test_helpers::{System, assert_timely_eq2, setup_rpc_client_and_server};

#[test]
fn search_receivable() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    // Get the list of wallet IDs already created
    let all_wallet_ids = node.wallet_services().wallets.wallet_ids();
    let wallet_id = all_wallet_ids[0];

    node.insert_into_wallet(&DEV_GENESIS_KEY);

    // Get initial balance before any operations
    let initial_balance = node.balance(&DEV_GENESIS_ACCOUNT);
    let mut lattice = UnsavedBlockLatticeBuilder::new();

    // Create a send block
    let receive_minimum = node.config.receive_minimum.clone();
    let send_amount = receive_minimum + Amount::raw(1);
    let block = lattice.genesis().send(&*DEV_GENESIS_KEY, send_amount);

    // Process the send block
    node.process_active(block);

    // Verify that balance was reduced
    assert_timely_eq2(
        || node.balance(&DEV_GENESIS_ACCOUNT),
        initial_balance - send_amount,
    );

    // Call search_receivable with the default wallet ID
    node.runtime.block_on(async {
        server.client.search_receivable(wallet_id).await.unwrap();
    });

    // Check that the balance has been updated
    let final_balance = node.runtime.block_on(async {
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            let balance = node.balance(&DEV_GENESIS_ACCOUNT);
            if balance == Amount::MAX || start.elapsed() > timeout {
                return balance;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });

    assert_eq!(final_balance, Amount::MAX);
}

#[test]
fn search_receivable_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime
        .block_on(async { server.client.search_receivable(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn search_receivable_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node
        .runtime
        .block_on(async { server.client.search_receivable(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
