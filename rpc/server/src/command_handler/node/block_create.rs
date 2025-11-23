use std::sync::Arc;

use anyhow::bail;

use rsnano_node::{Node, handles::LedgerQueryHandle};
use rsnano_rpc_messages::{BlockCreateArgs, BlockCreateResponse, BlockTypeDto};
use rsnano_types::{
    Account, Amount, Block, BlockDetails, BlockHash, ChangeBlockArgs, Epoch, OpenBlockArgs,
    PendingKey, PrivateKey, PublicKey, ReceiveBlockArgs, Root, SavedBlock, SendBlockArgs,
    StateBlockArgs, WorkNonce, WorkRequest,
};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn block_create(
        &self,
        args: BlockCreateArgs,
    ) -> anyhow::Result<BlockCreateResponse> {
        let difficulty = args
            .difficulty
            .unwrap_or_else(|| self.ledger_work_thresholds.threshold_base().into())
            .inner();

        let wallet_id = args.wallet.unwrap_or_default();
        let account = args.account.unwrap_or_default();
        let representative = PublicKey::from(args.representative.unwrap_or_default());
        let destination = args.destination.unwrap_or_default();
        let source = args.source.unwrap_or_default();
        let amount = args.balance.unwrap_or_default();
        let work: WorkNonce = args.work.unwrap_or(0.into());

        let mut previous = args.previous.unwrap_or(BlockHash::ZERO);
        let mut balance = args.balance.unwrap_or(Amount::ZERO);
        let mut prv_key = PrivateKey::zero();

        if work.is_zero() && !self.bootstrap.work_generation_enabled() {
            bail!("Work generation is disabled");
        }

        let ledger = self.ledger_queries.clone();

        if !wallet_id.is_zero() && !account.is_zero() {
            self.wallet_services.fetch(&wallet_id, &account.into())?;
            previous = ledger.account_head(&account).unwrap_or_default();
            balance = ledger.account_balance(&account);
        }

        if let Some(key) = args.key {
            prv_key = key.into();
        }

        let link = args.link.unwrap_or_else(|| {
            // Retrieve link from source or destination
            if source.is_zero() {
                destination.into()
            } else {
                source.into()
            }
        });

        // TODO block_response_put_l
        // TODO get_callback_l

        if prv_key.is_zero() {
            bail!("Private key or local wallet and account required");
        }
        let pub_key = prv_key.public_key();
        let account = Account::from(pub_key);
        // Fetching account balance & previous for send blocks (if aren't given directly)
        if args.previous.is_none() && args.balance.is_none() {
            previous = ledger.account_head(&account).unwrap_or_default();
            balance = ledger.account_balance(&account);
        }
        // Double check current balance if previous block is specified
        else if args.previous.is_some()
            && args.balance.is_some()
            && args.block_type == BlockTypeDto::Send
            && ledger.block_exists(&previous)
            && ledger.block_balance(&previous) != Some(balance)
        {
            bail!("Balance mismatch for previous block");
        }

        // Check for incorrect account key
        if let Some(acc) = args.account
            && acc != account
        {
            bail!("Incorrect key for given account");
        }

        let root: Root;
        let mut block = match args.block_type {
            BlockTypeDto::State => {
                if args.previous.is_some()
                    && !representative.is_zero()
                    && (!link.is_zero() || args.link.is_some())
                {
                    let block: Block = StateBlockArgs {
                        key: &prv_key,
                        previous,
                        representative,
                        balance,
                        link,
                        work,
                    }
                    .into();
                    if previous.is_zero() {
                        root = account.into();
                    } else {
                        root = previous.into();
                    }
                    block
                } else {
                    bail!(
                        "Previous, representative, final balance and link (source or destination) are required"
                    );
                }
            }
            BlockTypeDto::Open => {
                if !representative.is_zero() && !source.is_zero() {
                    let block: Block = OpenBlockArgs {
                        key: &prv_key,
                        source,
                        representative,
                        work,
                    }
                    .into();
                    root = account.into();
                    block
                } else {
                    bail!("Representative account and source hash required");
                }
            }
            BlockTypeDto::Receive => {
                if !source.is_zero() && !previous.is_zero() {
                    let block: Block = ReceiveBlockArgs {
                        key: &prv_key,
                        previous,
                        source,
                        work,
                    }
                    .into();
                    root = previous.into();
                    block
                } else {
                    bail!("Previous hash and source hash required");
                }
            }
            BlockTypeDto::Change => {
                if !representative.is_zero() && !previous.is_zero() {
                    let block: Block = ChangeBlockArgs {
                        key: &prv_key,
                        previous,
                        representative,
                        work,
                    }
                    .into();
                    root = previous.into();
                    block
                } else {
                    bail!("Representative account and previous hash required");
                }
            }
            BlockTypeDto::Send => {
                if !destination.is_zero()
                    && !previous.is_zero()
                    && !balance.is_zero()
                    && !amount.is_zero()
                {
                    if balance >= amount {
                        let block: Block = SendBlockArgs {
                            key: &prv_key,
                            previous,
                            destination,
                            balance: balance - amount,
                            work,
                        }
                        .into();
                        root = previous.into();
                        block
                    } else {
                        bail!("Insufficient balance")
                    }
                } else {
                    bail!(
                        "Destination account, previous hash, current balance and amount required"
                    );
                }
            }
            BlockTypeDto::Unknown => {
                bail!("Invalid block type");
            }
        };

        if work.is_zero() {
            // Difficulty calculation
            let difficulty = if args.difficulty.is_none() {
                difficulty_ledger(self.node.clone(), &ledger, &block)
            } else {
                difficulty
            };

            let work = match self
                .bootstrap
                .generate_work(WorkRequest::new(root, difficulty))
            {
                Some(work) => work,
                None => bail!("Work generation cancellation or failure"),
            };
            block.set_work(work);
        }

        let json_block = block.json_representation();
        Ok(BlockCreateResponse::new(
            block.hash(),
            self.node
                .network_params()
                .work
                .difficulty_block(&block)
                .into(),
            json_block,
        ))
    }
}

pub fn difficulty_ledger(node: Arc<Node>, ledger: &LedgerQueryHandle, block: &Block) -> u64 {
    let mut details = BlockDetails::new(Epoch::Epoch0, false, false, false);
    let mut details_found = false;

    // Previous block find
    let mut block_previous: Option<SavedBlock> = None;
    let previous = block.previous();
    if !previous.is_zero() {
        block_previous = ledger.get_block(&previous);
    }

    // Send check
    if block_previous.is_some() {
        let is_send =
            ledger.block_balance(&previous).unwrap_or_default() > block.balance_field().unwrap();
        details = BlockDetails::new(Epoch::Epoch0, is_send, false, false);
        details_found = true;
    }

    // Epoch check
    if let Some(prev_block) = &block_previous {
        let epoch = prev_block.epoch();
        details = BlockDetails::new(epoch, details.is_send, details.is_receive, details.is_epoch);
    }

    // Link check
    if let Some(link) = block.link_field()
        && !details.is_send
        && let Some(block_link) = ledger.get_block(&link.into())
    {
        let account = block.account_field().unwrap(); // Link is non-zero therefore it's a state block and has an account field;
        if ledger
            .get_pending(&PendingKey::new(account, link.into()))
            .is_some()
        {
            let epoch = std::cmp::max(details.epoch, block_link.epoch());
            details = BlockDetails::new(epoch, details.is_send, true, details.is_epoch);
            details_found = true;
        }
    }

    if details_found {
        node.network_params().work.threshold(&details)
    } else {
        node.network_params().work.threshold_base()
    }
}
