use rsnano_node::Node;

pub(crate) struct LedgerStats {
    pub total_blocks: u64,
    pub confirmed_blocks: u64,
    pub bps: i64,
    pub cps: i64,
}

impl LedgerStats {
    pub(crate) fn new() -> Self {
        Self {
            total_blocks: 0,
            confirmed_blocks: 0,
            bps: 0,
            cps: 0,
        }
    }

    pub(crate) fn update(&mut self, node: &Node) {
        self.total_blocks = node.services().ledger.block_count();
        self.confirmed_blocks = node.services().ledger.confirmed_count();
        self.bps = node.services().block_rates.bps();
        self.cps = node.services().block_rates.cps();
    }

    pub(crate) fn blocks_per_second(&self) -> i64 {
        self.bps
    }

    pub(crate) fn confirmations_per_second(&self) -> i64 {
        self.cps
    }
}
