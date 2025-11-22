use rsnano_node::handles::LedgerQueryHandle;
use rsnano_types::{Account, BlockHash, DetailedBlock};

pub(crate) struct Explorer {
    state: ExplorerState,
}

impl Explorer {
    pub(crate) fn new() -> Self {
        Self {
            state: ExplorerState::Empty,
        }
    }

    pub(crate) fn search(&mut self, ledger: &LedgerQueryHandle, input: &str) -> bool {
        if let Some(hash) = BlockHash::decode_hex(input.trim()) {
            self.state = match ledger.detailed_block(&hash) {
                Some(block) => ExplorerState::Block(block),
                None => ExplorerState::NotFound,
            };
            return true;
        };

        if let Some(account) = Account::parse(input) {
            self.state = if let Some(head) = ledger.account_head(&account) {
                match ledger.detailed_block(&head) {
                    Some(block) => ExplorerState::Block(block),
                    None => ExplorerState::NotFound,
                }
            } else {
                ExplorerState::NotFound
            };
            return true;
        }

        false
    }

    pub(crate) fn state(&self) -> &ExplorerState {
        &self.state
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum ExplorerState {
    Empty,
    NotFound,
    Block(DetailedBlock),
}
