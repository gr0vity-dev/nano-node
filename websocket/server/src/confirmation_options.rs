use std::collections::HashSet;

use serde_json::Value;
use tracing::warn;

use rsnano_node::WalletServices;
use rsnano_types::Account;
use rsnano_websocket_messages::ConfirmationJsonOptions;

pub trait WalletAccountLookup {
    fn account_exists(&self, account: &Account) -> bool;
}

impl WalletAccountLookup for WalletServices {
    fn account_exists(&self, account: &Account) -> bool {
        self.account_exists(&account.into())
    }
}

#[derive(Clone)]
pub struct ConfirmationOptions {
    pub include_election_info: bool,
    pub include_election_info_with_votes: bool,
    pub include_linked_account: bool,
    pub include_sideband_info: bool,
    pub include_block: bool,
    pub has_account_filtering_options: bool,
    pub all_local_accounts: bool,
    pub confirmation_types: u8,
    pub accounts: HashSet<String>,
}

impl ConfirmationOptions {
    const TYPE_ACTIVE_QUORUM: u8 = 1;
    const TYPE_ACTIVE_CONFIRMATION_HEIGHT: u8 = 2;
    const TYPE_INACTIVE: u8 = 4;
    const TYPE_ALL_ACTIVE: u8 = Self::TYPE_ACTIVE_QUORUM | Self::TYPE_ACTIVE_CONFIRMATION_HEIGHT;
    const TYPE_ALL: u8 = Self::TYPE_ALL_ACTIVE | Self::TYPE_INACTIVE;

    pub fn new(options: ConfirmationJsonOptions) -> Self {
        let mut result = Self {
            include_election_info: false,
            include_election_info_with_votes: false,
            include_sideband_info: false,
            include_block: true,
            include_linked_account: false,
            has_account_filtering_options: false,
            all_local_accounts: false,
            confirmation_types: Self::TYPE_ALL,
            accounts: HashSet::new(),
        };
        // Non-account filtering options
        result.include_block = options.include_block.unwrap_or(true);
        result.include_election_info = options.include_election_info.unwrap_or(false);
        result.include_election_info_with_votes =
            options.include_election_info_with_votes.unwrap_or(false);
        result.include_linked_account = options.include_linked_account.unwrap_or(false);
        result.include_sideband_info = options.include_sideband_info.unwrap_or(false);

        let conf_type = options
            .confirmation_type
            .unwrap_or_else(|| "all".to_string());

        if conf_type.eq_ignore_ascii_case("active") {
            result.confirmation_types = Self::TYPE_ALL_ACTIVE;
        } else if conf_type.eq_ignore_ascii_case("active_quorum") {
            result.confirmation_types = Self::TYPE_ACTIVE_QUORUM;
        } else if conf_type.eq_ignore_ascii_case("active_confirmation_height") {
            result.confirmation_types = Self::TYPE_ACTIVE_CONFIRMATION_HEIGHT;
        } else if conf_type.eq_ignore_ascii_case("inactive") {
            result.confirmation_types = Self::TYPE_INACTIVE;
        } else {
            result.confirmation_types = Self::TYPE_ALL;
        }

        // Account filtering options
        let all_local_accounts = options.all_local_accounts.unwrap_or(false);
        if all_local_accounts {
            result.all_local_accounts = true;
            result.has_account_filtering_options = true;
            if !result.include_block {
                warn!(
                    "Websocket: Filtering option \"all_local_accounts\" requires that \"include_block\" is set to true to be effective"
                );
            }
        }
        if let Some(accounts) = options.accounts {
            result.has_account_filtering_options = true;
            for account in accounts {
                match Account::parse(&account) {
                    Some(acc) => {
                        // Do not insert the given raw data to keep old prefix support
                        result.accounts.insert(acc.encode_account());
                    }
                    None => {
                        warn!("Invalid account provided for filtering blocks: {}", account);
                    }
                }
            }

            if !result.include_block {
                warn!(
                    "Filtering option \"accounts\" requires that \"include_block\" is set to true to be effective"
                );
            }
        }
        result.check_filter_empty();

        if result.include_linked_account && !result.include_block {
            warn!(
                "The option \"include_linked_account\" requires \"include_block\" to be set to true, as linked accounts are only retrieved when block content is included"
            )
        }

        result
    }

    /**
     * Checks if a message should be filtered for given block confirmation options.
     * @param message_a the message to be checked
     * @return false if the message should be broadcasted, true if it should be filtered
     */
    pub fn should_filter<L: WalletAccountLookup>(
        &self,
        message_content: &Value,
        wallets: &L,
    ) -> bool {
        let mut should_filter_conf_type = true;

        if let Some(serde_json::Value::String(type_text)) = message_content.get("confirmation_type")
        {
            let confirmation_types = self.confirmation_types;
            if (type_text == "active_quorum" && (confirmation_types & Self::TYPE_ACTIVE_QUORUM) > 0)
                || (type_text == "active_confirmation_height"
                    && (confirmation_types & Self::TYPE_ACTIVE_CONFIRMATION_HEIGHT) > 0)
                || (type_text == "inactive" && (confirmation_types & Self::TYPE_INACTIVE) > 0)
            {
                should_filter_conf_type = false;
            }
        }

        let mut should_filter_account = self.has_account_filtering_options;
        if let Some(serde_json::Value::Object(block)) = message_content.get("block")
            && let Some(serde_json::Value::String(destination_text)) = block.get("link_as_account")
        {
            let source_text = match message_content.get("account") {
                Some(serde_json::Value::String(s)) => s.as_str(),
                _ => "",
            };
            if self.all_local_accounts {
                let source = Account::parse(source_text).unwrap_or_default();
                let destination = Account::parse(destination_text).unwrap_or_default();
                if wallets.account_exists(&source) || wallets.account_exists(&destination) {
                    should_filter_account = false;
                }
            }
            if self.accounts.contains(source_text) || self.accounts.contains(destination_text) {
                should_filter_account = false;
            }
        }

        should_filter_conf_type || should_filter_account
    }

    /**
     * Update some existing options
     * Filtering options:
     * - "accounts_add" (array of std::strings) - additional accounts for which blocks should not be filtered
     * - "accounts_del" (array of std::strings) - accounts for which blocks should be filtered
     */
    pub fn update(&mut self, options: &serde_json::Value) {
        let mut update_accounts = |accounts_text: &serde_json::Value, insert: bool| {
            self.has_account_filtering_options = true;
            if let serde_json::Value::Array(accounts) = accounts_text {
                for account in accounts {
                    if let serde_json::Value::String(acc_str) = account {
                        match Account::parse(acc_str) {
                            Some(result) => {
                                // Re-encode to keep old prefix support
                                let encoded = result.encode_account();
                                if insert {
                                    self.accounts.insert(encoded);
                                } else {
                                    self.accounts.remove(&encoded);
                                }
                            }
                            None => {
                                warn!("Invalid account provided for filtering blocks: {}", acc_str);
                            }
                        }
                    }
                }
            }
        };

        // Adding accounts as filter exceptions
        if let serde_json::Value::Object(obj) = options {
            if let Some(accounts_add) = obj.get("accounts_add") {
                update_accounts(accounts_add, true);
            }

            // Removing accounts as filter exceptions
            if let Some(accounts_del) = obj.get("accounts_del") {
                update_accounts(accounts_del, false);
            }
        }

        self.check_filter_empty();
    }

    pub fn check_filter_empty(&self) {
        // Warn the user if the options resulted in an empty filter
        if self.has_account_filtering_options
            && !self.all_local_accounts
            && self.accounts.is_empty()
        {
            warn!("Provided options resulted in an empty account confirmation filter");
        }
    }
}
