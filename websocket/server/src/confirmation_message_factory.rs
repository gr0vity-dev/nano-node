use rsnano_node::{consensus::election::ConfirmedElection, handles::LedgerQueryHandle};
use rsnano_types::{Amount, BlockType, SavedBlock};
use rsnano_websocket_messages::{
    BlockConfirmed, ElectionInfo, JsonSideband, MessageEnvelope, Topic,
};

use crate::{ConfirmationOptions, into_election_info, into_json_sideband, into_json_vote_summary};

pub(super) struct ConfirmationMessageFactory<'a> {
    pub ledger: &'a LedgerQueryHandle,
    pub options: &'a ConfirmationOptions,
    pub block: &'a SavedBlock,
    pub amount: &'a Amount,
    pub election: &'a ConfirmedElection,
}

impl ConfirmationMessageFactory<'_> {
    pub fn create_message(&self) -> MessageEnvelope {
        MessageEnvelope::new(
            Topic::Confirmation,
            BlockConfirmed {
                account: self.block.account().encode_account(),
                amount: self.amount.to_string_dec(),
                hash: self.block.hash().to_string(),
                confirmation_type: self.confirmation_type(),
                election_info: self.election_info(),
                block: self.json_block(),
                sideband: self.sideband(),
                linked_account: self.linked_account(),
            },
        )
    }

    fn confirmation_type(&self) -> String {
        self.election.confirmation_type.as_str().to_string()
    }

    fn subtype(&self) -> String {
        if self.block.block_type() == BlockType::State {
            self.block.subtype().as_str().to_string()
        } else {
            String::new()
        }
    }

    fn election_info(&self) -> Option<ElectionInfo> {
        if self.options.include_election_info || self.options.include_election_info_with_votes {
            let mut info = into_election_info(self.election);
            if self.options.include_election_info_with_votes {
                let mut votes: Vec<_> = self.election.votes.values().cloned().collect();
                // sort by descending weight
                votes.sort_by(|a, b| b.weight.cmp(&a.weight));
                info.votes = Some(votes.iter().map(into_json_vote_summary).collect());
            }
            Some(info)
        } else {
            None
        }
    }

    fn json_block(&self) -> Option<serde_json::Value> {
        if self.options.include_block {
            let mut block_value: serde_json::Value = (**self.block).clone().into();
            let subtype = self.subtype();
            if !subtype.is_empty()
                && let serde_json::Value::Object(o) = &mut block_value
            {
                o.insert("subtype".to_string(), serde_json::Value::String(subtype));
            }

            Some(block_value)
        } else {
            None
        }
    }

    fn linked_account(&self) -> Option<String> {
        if !self.options.include_block || !self.options.include_linked_account {
            return None;
        }

        match self.ledger.linked_account(self.block) {
            Some(linked) => Some(linked.encode_account()),
            None => Some("0".to_owned()),
        }
    }

    fn sideband(&self) -> Option<JsonSideband> {
        if self.options.include_sideband_info {
            Some(into_json_sideband(self.block.sideband()))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rsnano_ledger::Ledger;
    use rsnano_node::consensus::election::ConfirmationType;
    use rsnano_node::handles::LedgerQueryHandle;
    use rsnano_websocket_messages::ConfirmationJsonOptions;
    use store_rocksdb::default_ledger_store_factory;

    use super::*;

    fn new_ledger() -> Ledger {
        Ledger::new_null(default_ledger_store_factory())
    }

    #[test]
    fn default_options() {
        let ledger = new_ledger();
        let options = ConfirmationOptions::new(ConfirmationJsonOptions::default());
        let block = SavedBlock::new_test_instance();
        let amount = Amount::nano(123);
        let mut election =
            ConfirmedElection::new(block.clone(), ConfirmationType::InactiveConfirmationHeight);
        election.confirmation_type = ConfirmationType::InactiveConfirmationHeight;
        let factory = ConfirmationMessageFactory {
            ledger: &LedgerQueryHandle::new(Arc::new(ledger)),
            options: &options,
            block: &block,
            amount: &amount,
            election: &election,
        };

        let message = factory.create_message();

        assert_eq!(message.topic, Some(Topic::Confirmation));
        let payload: BlockConfirmed = serde_json::from_value(message.message.unwrap()).unwrap();
        assert_eq!(payload.amount, amount.to_string_dec());
        assert_eq!(payload.hash, block.hash().to_string());
        assert_eq!(payload.confirmation_type, "inactive");
        assert!(payload.election_info.is_none());
        assert!(payload.block.is_some());
        assert!(payload.sideband.is_none());
        assert!(payload.linked_account.is_none());
    }

    #[test]
    fn linked_account() {
        let ledger = new_ledger();
        let options = ConfirmationOptions::new(ConfirmationJsonOptions {
            include_block: Some(true),
            include_linked_account: Some(true),
            ..Default::default()
        });
        let block = SavedBlock::new_test_send_block();
        let amount = Amount::nano(123);
        let factory = ConfirmationMessageFactory {
            ledger: &LedgerQueryHandle::new(Arc::new(ledger)),
            options: &options,
            block: &block,
            amount: &amount,
            election: &ConfirmedElection::new(
                block.clone(),
                ConfirmationType::InactiveConfirmationHeight,
            ),
        };

        let message = factory.create_message();

        let payload: BlockConfirmed = serde_json::from_value(message.message.unwrap()).unwrap();

        assert_eq!(
            payload.linked_account,
            Some(block.destination_or_link().encode_account())
        );
    }
}
