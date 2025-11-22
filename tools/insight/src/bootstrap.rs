use rsnano_node::bootstrap::state::{BlockingEntry, BootstrapLogicSnapshot, Priority};
use rsnano_types::Account;

#[derive(Default)]
pub(crate) struct BootstrapInfo {
    pub priority_accounts: usize,
    pub blocked_accounts: usize,
    pub unique_blocking_accounts: usize,
    pub known_dependencies: usize,
    pub priorities: Vec<(Priority, Account)>,
    pub blocked: Vec<BlockingEntry>,
    pub search: String,
    pub add_account: String,
}

impl BootstrapInfo {
    pub(crate) fn update(&mut self, state: &BootstrapLogicSnapshot) {
        let target_account = Account::parse(&self.search);
        let candidates = &state.candidate_accounts;
        self.priority_accounts = candidates.priority_len;
        self.blocked_accounts = candidates.blocked_len;
        self.unique_blocking_accounts = candidates.unique_blocking_accounts;
        self.known_dependencies = candidates.known_dependencies;

        self.priorities = candidates
            .priorities
            .iter()
            .filter_map(|(prio, acc)| {
                if target_account.is_none() || target_account.as_ref() == Some(acc) {
                    Some((*prio, *acc))
                } else {
                    None
                }
            })
            .take(50)
            .collect();

        self.blocked = candidates
            .blocked
            .iter()
            .filter(|i| {
                target_account.is_none()
                    || target_account == Some(i.account)
                    || target_account == Some(i.dependency_account)
            })
            .take(50)
            .cloned()
            .collect();
    }
}
