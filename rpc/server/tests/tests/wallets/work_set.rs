#[cfg(test)]
mod tests {
    use rsnano_types::{Account, WalletId};
    use std::time::Duration;
    use test_helpers::{System, assert_timely, setup_rpc_client_and_server};

    #[test]
    fn work_set() {
        let mut system = System::new();
        let node = system.make_node();

        let server = setup_rpc_client_and_server(node.clone(), true);

        let wallet_id = WalletId::random();
        node.services.wallets.create(wallet_id);

        node.runtime.block_on(async {
            server
                .client
                .work_set(wallet_id, Account::ZERO, 1.into())
                .await
                .unwrap()
        });

        assert_timely(Duration::from_secs(5), || {
            !node
                .services
                .wallets
                .work_get2(&wallet_id, &Account::ZERO.into())
                .unwrap()
                .is_zero()
        });
    }

    #[test]
    fn work_set_fails_without_enable_control() {
        let mut system = System::new();
        let node = system.make_node();

        let server = setup_rpc_client_and_server(node.clone(), false);

        let result = node.runtime.block_on(async {
            server
                .client
                .work_set(WalletId::random(), Account::ZERO, 1.into())
                .await
        });

        assert_eq!(
            result.err().map(|e| e.to_string()),
            Some("node returned error: \"RPC control is disabled\"".to_string())
        );
    }

    #[test]
    fn work_set_fails_with_wallet_not_found() {
        let mut system = System::new();
        let node = system.make_node();

        let server = setup_rpc_client_and_server(node.clone(), true);

        let result = node.runtime.block_on(async {
            server
                .client
                .work_set(WalletId::random(), Account::ZERO, 1.into())
                .await
        });

        assert_eq!(
            result.err().map(|e| e.to_string()),
            Some("node returned error: \"Wallet not found\"".to_string())
        );
    }
}
