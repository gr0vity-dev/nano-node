use std::sync::Arc;

#[cfg(test)]
use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{AnySet, ConfirmedSet, Ledger, LedgerSet};
use rsnano_messages::{AscPullReqType, BlocksReqPayload, HashType};
use rsnano_network::Channel;
use rsnano_types::{Account, BlockHash, HashOrAccount};

use super::{
    pull_count_decider::PullCountDecider,
    pull_type_decider::{PullType, PullTypeDecider},
};
use crate::bootstrap::{AscPullQuerySpec, PromiseContext};
use tracing::trace;

/// Creates a query for the next priority account
pub(super) struct QueryFactory {
    ledger: Arc<Ledger>,
    pull_type_decider: PullTypeDecider,
    pull_count_decider: PullCountDecider,
}

impl QueryFactory {
    pub(super) fn new(
        ledger: Arc<Ledger>,
        pull_type_decider: PullTypeDecider,
        pull_count_decider: PullCountDecider,
    ) -> Self {
        Self {
            ledger,
            pull_type_decider,
            pull_count_decider,
        }
    }

    pub fn next_priority_query(
        &mut self,
        context: &mut PromiseContext,
        channel: Arc<Channel>,
    ) -> Option<AscPullQuerySpec> {
        let next = context.logic.next_priority(context.now);

        if next.account.is_zero() {
            return None;
        }

        let (head, confirmed_frontier) = self.get_account_info(&next.account);
        let pull_type = self.pull_type_decider.decide_pull_type();

        let pull_start = PullStart::new(pull_type, next.account, head, confirmed_frontier);
        let req_type = AscPullReqType::Blocks(BlocksReqPayload {
            start_type: pull_start.start_type,
            start: pull_start.start,
            count: (&self).pull_count_decider.pull_count((&next).priority),
        });

        // Only cooldown accounts that are likely to have more blocks
        // This is to avoid requesting blocks from the same frontier multiple times, before the block processor had a chance to process them
        // Not throttling accounts that are probably up-to-date allows us to evict them from the priority set faster
        let cooldown_account = next.fails == 0;

        let query_spec = AscPullQuerySpec {
            query_id: context.id,
            channel: channel,
            req_type,
            hash: pull_start.hash,
            account: (&next).account,
            cooldown_account,
        };
        trace!(query_id = context.id, ?pull_type, "Created pull query spec");

        Some(query_spec)
    }

    fn get_account_info(&self, account: &Account) -> (BlockHash, BlockHash) {
        let any = self.ledger.any();
        let account_info = any.get_account(account);
        let head = account_info.map(|i| i.head).unwrap_or_default();

        if let Some(conf_info) = any.confirmed().get_conf_info(account) {
            (head, conf_info.frontier)
        } else {
            (head, BlockHash::ZERO)
        }
    }
}

struct PullStart {
    start: HashOrAccount,
    start_type: HashType,
    hash: BlockHash,
}

impl PullStart {
    fn new(
        pull_type: PullType,
        account: Account,
        head: BlockHash,
        confirmed_frontier: BlockHash,
    ) -> Self {
        // Check if the account picked has blocks, if it does, start the pull from the highest block
        if head.is_zero() {
            PullStart::account(account)
        } else {
            match pull_type {
                PullType::Optimistic => PullStart::block(head),
                PullType::Safe => PullStart::safe(account, confirmed_frontier),
            }
        }
    }

    fn safe(account: Account, confirmed_frontier: BlockHash) -> Self {
        if confirmed_frontier.is_zero() {
            PullStart::account(account)
        } else {
            PullStart::block(confirmed_frontier)
        }
    }

    fn account(account: Account) -> Self {
        Self {
            start: account.into(),
            start_type: HashType::Account,
            hash: BlockHash::ZERO,
        }
    }

    fn block(start: BlockHash) -> Self {
        Self {
            start: start.into(),
            start_type: HashType::Block,
            hash: start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::state::BootstrapLogic;
    use rsnano_types::{AccountInfo, ConfirmationHeightInfo};

    #[test]
    fn empty() {
        let query = create_query(&TestInput {
            prioritized_account: None,
            head: None,
            confirmed: None,
            pull_type: PullType::Optimistic,
            query_id: 42,
        });

        assert!(query.is_none());
    }

    mod optimistic {
        use super::*;

        #[test]
        fn account_not_in_ledger() {
            let account = Account::from(42);

            let input = TestInput {
                prioritized_account: Some(account),
                head: None,
                confirmed: None,
                pull_type: PullType::Optimistic,
                query_id: 42,
            };

            let query = create_query(&input).unwrap();

            assert_eq!(
                query,
                AscPullQuerySpec {
                    query_id: input.query_id,
                    channel: test_channel(),
                    account,
                    hash: BlockHash::ZERO,
                    cooldown_account: true,
                    req_type: AscPullReqType::Blocks(BlocksReqPayload {
                        start_type: HashType::Account,
                        start: account.into(),
                        count: 2
                    })
                }
            );
        }

        #[test]
        fn account_in_ledger() {
            let account = Account::from(42);
            let head = BlockHash::from(7);

            let input = TestInput {
                prioritized_account: Some(account),
                head: Some(head),
                confirmed: None,
                pull_type: PullType::Optimistic,
                query_id: 42,
            };

            let query = create_query(&input).unwrap();

            assert_eq!(
                query,
                AscPullQuerySpec {
                    query_id: input.query_id,
                    channel: test_channel(),
                    account,
                    hash: head,
                    cooldown_account: true,
                    req_type: AscPullReqType::Blocks(BlocksReqPayload {
                        start_type: HashType::Block,
                        start: head.into(),
                        count: 2
                    })
                }
            );
        }
    }

    mod safe {
        use super::*;

        #[test]
        fn account_not_in_ledger() {
            let account = Account::from(42);
            let input = TestInput {
                prioritized_account: Some(account),
                head: None,
                confirmed: None,
                pull_type: PullType::Safe,
                query_id: 42,
            };
            let query = create_query(&input).unwrap();

            assert_eq!(
                query,
                AscPullQuerySpec {
                    query_id: input.query_id,
                    channel: test_channel(),
                    account,
                    hash: BlockHash::ZERO,
                    cooldown_account: true,
                    req_type: AscPullReqType::Blocks(BlocksReqPayload {
                        start_type: HashType::Account,
                        start: account.into(),
                        count: 2
                    })
                }
            );
        }

        #[test]
        fn account_in_ledger_and_confirmed() {
            let account = Account::from(42);
            let frontier = BlockHash::from(7);

            let input = TestInput {
                prioritized_account: Some(account),
                head: Some(BlockHash::from(111)),
                confirmed: Some(frontier),
                pull_type: PullType::Safe,
                query_id: 42,
            };

            let query = create_query(&input).unwrap();

            assert_eq!(
                query,
                AscPullQuerySpec {
                    query_id: input.query_id,
                    channel: test_channel(),
                    account,
                    hash: frontier,
                    cooldown_account: true,
                    req_type: AscPullReqType::Blocks(BlocksReqPayload {
                        start_type: HashType::Block,
                        start: frontier.into(),
                        count: 2
                    })
                }
            );
        }

        #[test]
        fn account_in_ledger_and_unconfirmed() {
            let account = Account::from(42);

            let input = TestInput {
                prioritized_account: Some(account),
                head: Some(BlockHash::from(111)),
                confirmed: None,
                pull_type: PullType::Safe,
                query_id: 42,
            };

            let query = create_query(&input).unwrap();

            assert_eq!(
                query,
                AscPullQuerySpec {
                    query_id: input.query_id,
                    channel: test_channel(),
                    account,
                    hash: BlockHash::ZERO,
                    cooldown_account: true,
                    req_type: AscPullReqType::Blocks(BlocksReqPayload {
                        start_type: HashType::Account,
                        start: account.into(),
                        count: 2
                    })
                }
            );
        }
    }

    fn create_query(input: &TestInput) -> Option<AscPullQuerySpec> {
        let account = input.prioritized_account.unwrap_or_default();
        let ledger = create_ledger(account, input.head, input.confirmed);
        let pull_type_decider = PullTypeDecider::new_null_with(input.pull_type);
        let pull_count_decider = PullCountDecider::default();
        let mut factory = QueryFactory::new(ledger, pull_type_decider, pull_count_decider);
        let mut state = BootstrapLogic::default();

        if let Some(account) = &input.prioritized_account {
            state.candidate_accounts.priority_up(account);
        }

        let mut context = PromiseContext::new_test_instance(&mut state);
        context.id = input.query_id;
        factory.next_priority_query(&mut context, test_channel())
    }

    fn create_ledger(
        account: Account,
        head: Option<BlockHash>,
        confirmed: Option<BlockHash>,
    ) -> Arc<Ledger> {
        let mut ledger_builder = Ledger::new_null_builder(default_ledger_store_factory());

        if let Some(head) = head {
            ledger_builder = ledger_builder.account_info(
                &account,
                &AccountInfo {
                    head,
                    ..Default::default()
                },
            );
        }

        if let Some(frontier) = confirmed {
            ledger_builder = ledger_builder.confirmation_height(
                &account,
                &ConfirmationHeightInfo {
                    height: 123,
                    frontier,
                },
            )
        }

        Arc::new(ledger_builder.finish())
    }

    struct TestInput {
        prioritized_account: Option<Account>,
        head: Option<BlockHash>,
        confirmed: Option<BlockHash>,
        pull_type: PullType,
        query_id: u64,
    }

    fn test_channel() -> Arc<Channel> {
        Arc::new(Channel::new_test_instance())
    }
}
