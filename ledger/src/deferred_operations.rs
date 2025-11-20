use crate::Ledger;
use rsnano_types::{Amount, PublicKey};

pub struct DeferredLedgerOperations {
    rep_weight_ops: Vec<RepWeightOp>,
    custom: Vec<Box<dyn FnOnce(&Ledger) + Send>>,
}

enum RepWeightOp {
    Add {
        representative: PublicKey,
        amount: Amount,
    },
    AddDual {
        rep_1: PublicKey,
        amount_1: Amount,
        rep_2: PublicKey,
        amount_2: Amount,
    },
}

impl DeferredLedgerOperations {
    pub fn new() -> Self {
        Self {
            rep_weight_ops: Vec::new(),
            custom: Vec::new(),
        }
    }

    pub fn add_rep_weight(&mut self, representative: PublicKey, amount: Amount) {
        self.rep_weight_ops.push(RepWeightOp::Add {
            representative,
            amount,
        });
    }

    pub fn add_rep_weight_dual(
        &mut self,
        rep_1: PublicKey,
        amount_1: Amount,
        rep_2: PublicKey,
        amount_2: Amount,
    ) {
        self.rep_weight_ops.push(RepWeightOp::AddDual {
            rep_1,
            amount_1,
            rep_2,
            amount_2,
        });
    }

    pub fn add_custom<F>(&mut self, op: F)
    where
        F: FnOnce(&Ledger) + Send + 'static,
    {
        self.custom.push(Box::new(op));
    }

    pub fn execute(self, ledger: &Ledger) {
        self.execute_rep_weight_ops(ledger);
        for op in self.custom {
            op(ledger);
        }
    }

    fn execute_rep_weight_ops(&self, ledger: &Ledger) {
        if self.rep_weight_ops.is_empty() {
            return;
        }

        ledger.apply_rep_weight_ops(|txn| {
            for op in &self.rep_weight_ops {
                match op {
                    RepWeightOp::Add {
                        representative,
                        amount,
                    } => {
                        ledger
                            .rep_weights_updater
                            .representation_add(txn, *representative, *amount)
                    }
                    RepWeightOp::AddDual {
                        rep_1,
                        amount_1,
                        rep_2,
                        amount_2,
                    } => ledger
                        .rep_weights_updater
                        .representation_add_dual(txn, *rep_1, *amount_1, *rep_2, *amount_2),
                }
            }
        });
    }
}
