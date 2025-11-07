use std::{collections::HashMap, time::Duration};

use rsnano_types::{
    Account, Amount, Block, BlockDetails, BlockHash, BlockSideband, ChangeBlockArgs,
    DEV_GENESIS_BLOCK, DEV_GENESIS_KEY, Epoch, EpochBlockArgs, Link, OpenBlockArgs, PendingInfo,
    PendingKey, PrivateKey, PublicKey, ReceiveBlockArgs, Root, SavedBlock, SendBlockArgs,
    StateBlockArgs, UnixMillisTimestamp, WorkNonce, dev_epoch1_signer, epoch_v1_link,
};
use rsnano_work_validation::{WorkThresholds, dev_difficulty};

pub struct SavedBlockLatticeBuilder {
    accounts: HashMap<Account, Frontier>,
    work_generator: WorkGenerator,
    pending_receives: HashMap<PendingKey, PendingInfo>,
    now: UnixMillisTimestamp,
}

#[derive(Clone, Default)]
struct Frontier {
    hash: BlockHash,
    representative: PublicKey,
    balance: Amount,
    height: u64,
}

impl SavedBlockLatticeBuilder {
    pub fn new() -> Self {
        Self::with_work_generator(WorkGenerator::dev_network())
    }

    pub fn with_stub_work() -> Self {
        Self::with_work_generator(WorkGenerator::stub(WorkNonce::new(42)))
    }

    fn with_work_generator(work_generator: WorkGenerator) -> Self {
        let mut accounts = HashMap::new();
        accounts.insert(
            DEV_GENESIS_KEY.account(),
            Frontier {
                hash: DEV_GENESIS_BLOCK.hash(),
                representative: DEV_GENESIS_KEY.public_key(),
                balance: Amount::MAX,
                height: 1,
            },
        );
        Self {
            accounts,
            work_generator,
            pending_receives: Default::default(),
            now: UnixMillisTimestamp::new(42000),
        }
    }

    pub fn set_now(&mut self, now: UnixMillisTimestamp) {
        self.now = now;
    }

    pub fn advance_time(&mut self) {
        self.now = self.now + Duration::from_secs(1);
    }

    pub fn genesis(&mut self) -> SavedAccountChainBuilder<'_> {
        self.account(&DEV_GENESIS_KEY)
    }

    pub fn account<'a>(&'a mut self, key: &'a PrivateKey) -> SavedAccountChainBuilder<'a> {
        SavedAccountChainBuilder { lattice: self, key }
    }

    fn create_pow(&self, root: impl Into<Root>) -> WorkNonce {
        self.work_generator.generate(root.into())
    }

    pub fn epoch_open(&mut self, account: impl Into<Account>) -> SavedBlock {
        let account = account.into();
        assert!(!self.accounts.contains_key(&account));
        assert!(
            self.pending_receives
                .keys()
                .any(|k| k.receiving_account == account)
        );

        let receive: Block = EpochBlockArgs {
            epoch_signer: dev_epoch1_signer(),
            account,
            previous: BlockHash::ZERO,
            representative: PublicKey::ZERO,
            balance: Amount::ZERO,
            link: epoch_v1_link(),
            work: self.create_pow(account),
        }
        .into();

        let new_frontier = Frontier {
            hash: receive.hash(),
            representative: PublicKey::ZERO,
            balance: Amount::ZERO,
            height: 1,
        };

        let sideband = self.sideband_for(account, &Frontier::default(), &new_frontier);

        self.accounts.insert(account, new_frontier);

        SavedBlock::new(receive, sideband)
    }

    fn sideband_for(
        &self,
        account: Account,
        old_frontier: &Frontier,
        new_frontier: &Frontier,
    ) -> BlockSideband {
        let is_send = new_frontier.balance < old_frontier.balance;
        let is_receive = new_frontier.balance > old_frontier.balance;

        BlockSideband {
            height: new_frontier.height,
            timestamp: self.now,
            account,
            balance: new_frontier.balance,
            details: BlockDetails::new(Epoch::Epoch0, is_send, is_receive, false), //TODO epoch
            source_epoch: Epoch::Epoch0,                                           //TODO
        }
    }

    fn pop_pending_receive(
        &mut self,
        receiving_account: impl Into<Account>,
        send_hash: BlockHash,
    ) -> PendingInfo {
        self.pending_receives
            .remove(&PendingKey::new(receiving_account.into(), send_hash))
            .expect("no pending receive found")
    }
}

impl Clone for SavedBlockLatticeBuilder {
    fn clone(&self) -> Self {
        Self {
            accounts: self.accounts.clone(),
            work_generator: self.work_generator.clone(),
            pending_receives: self.pending_receives.clone(),
            now: self.now,
        }
    }
}

pub struct SavedAccountChainBuilder<'a> {
    lattice: &'a mut SavedBlockLatticeBuilder,
    key: &'a PrivateKey,
}

impl<'a> SavedAccountChainBuilder<'a> {
    fn create_pow(&self, root: impl Into<Root>) -> WorkNonce {
        self.lattice.create_pow(root)
    }

    pub fn send_max(&mut self, destination: impl Into<Account>) -> SavedBlock {
        self.send_all_except(destination, 0)
    }

    pub fn send_all_except(
        &mut self,
        destination: impl Into<Account>,
        keep: impl Into<Amount>,
    ) -> SavedBlock {
        let frontier = self.get_frontier();
        self.send(destination, frontier.balance - keep.into())
    }

    pub fn send(
        &mut self,
        destination: impl Into<Account>,
        amount: impl Into<Amount>,
    ) -> SavedBlock {
        let destination = destination.into();
        let old_frontier = self.get_frontier();
        let amount = amount.into();
        let new_balance = old_frontier.balance - amount;

        let send: Block = StateBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            representative: old_frontier.representative,
            balance: new_balance,
            link: destination.into(),
            work: self.create_pow(old_frontier.hash),
        }
        .into();

        let new_frontier = Frontier {
            hash: send.hash(),
            representative: old_frontier.representative,
            balance: new_balance,
            height: old_frontier.height + 1,
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);
        self.lattice.pending_receives.insert(
            PendingKey::new(destination, send.hash()),
            PendingInfo {
                source: self.key.account(),
                amount,
                epoch: Epoch::Epoch0,
            },
        );

        SavedBlock::new(send, sideband)
    }

    pub fn legacy_send(
        &mut self,
        destination: impl Into<Account>,
        amount: impl Into<Amount>,
    ) -> SavedBlock {
        let destination = destination.into();
        let old_frontier = self.get_frontier();
        let amount = amount.into();
        let new_balance = old_frontier.balance - amount;
        let work = self.create_pow(old_frontier.hash);

        let send: Block = SendBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            destination,
            balance: new_balance,
            work,
        }
        .into();

        let new_frontier = Frontier {
            hash: send.hash(),
            balance: new_balance,
            height: old_frontier.height,
            ..old_frontier
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);

        self.lattice.pending_receives.insert(
            PendingKey::new(destination, send.hash()),
            PendingInfo {
                source: self.key.account(),
                amount,
                epoch: Epoch::Epoch0,
            },
        );

        SavedBlock::new(send, sideband)
    }

    pub fn legacy_open(&mut self, corresponding_send: &Block) -> SavedBlock {
        self.legacy_open_with_rep(corresponding_send, self.key.public_key())
    }

    pub fn legacy_open_with_rep(
        &mut self,
        corresponding_send: &Block,
        new_representative: impl Into<PublicKey>,
    ) -> SavedBlock {
        assert!(!self.lattice.accounts.contains_key(&self.key.account()));
        assert_eq!(corresponding_send.destination_or_link(), self.key.account());

        let amount = self
            .lattice
            .pop_pending_receive(self.key, corresponding_send.hash())
            .amount;

        let root: Root = self.key.account().into();

        let work = self.create_pow(root);
        let receive: Block = OpenBlockArgs {
            key: &self.key,
            source: corresponding_send.hash(),
            representative: new_representative.into(),
            work,
        }
        .into();

        let old_frontier = Frontier::default();

        let new_frontier = Frontier {
            hash: receive.hash(),
            representative: self.key.public_key(),
            balance: amount,
            height: 1,
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);

        SavedBlock::new(receive, sideband)
    }

    pub fn legacy_receive(&mut self, corresponding_send: &Block) -> SavedBlock {
        let frontier = self.get_frontier();
        self.legacy_receive_with_rep(corresponding_send, frontier.representative)
    }

    pub fn legacy_receive_with_rep(
        &mut self,
        corresponding_send: &Block,
        new_representative: impl Into<PublicKey>,
    ) -> SavedBlock {
        assert_eq!(corresponding_send.destination_or_link(), self.key.account());
        let amount = self
            .lattice
            .pop_pending_receive(self.key, corresponding_send.hash())
            .amount;

        let old_frontier = self.get_frontier();
        let root: Root = old_frontier.hash.into();
        let new_balance = old_frontier.balance + amount;
        let work = self.create_pow(root);

        let receive: Block = ReceiveBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            source: corresponding_send.hash(),
            work,
        }
        .into();

        let new_frontier = Frontier {
            hash: receive.hash(),
            representative: new_representative.into(),
            balance: new_balance,
            height: old_frontier.height + 1,
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);

        SavedBlock::new(receive, sideband)
    }

    pub fn receive(&mut self, corresponding_send: &Block) -> SavedBlock {
        let frontier = self.get_frontier_or_empty();
        self.receive_and_change(corresponding_send, frontier.representative)
    }

    pub fn receive_and_change(
        &mut self,
        corresponding_send: &Block,
        new_representative: impl Into<PublicKey>,
    ) -> SavedBlock {
        assert_eq!(corresponding_send.destination_or_link(), self.key.account());
        let amount = self
            .lattice
            .pop_pending_receive(self.key, corresponding_send.hash())
            .amount;

        let old_frontier = self.get_frontier_or_empty();

        let root: Root = if old_frontier.hash.is_zero() {
            self.key.account().into()
        } else {
            old_frontier.hash.into()
        };

        let new_balance = old_frontier.balance + amount;

        let receive: Block = StateBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            representative: new_representative.into(),
            balance: new_balance,
            link: corresponding_send.hash().into(),
            work: self.create_pow(root),
        }
        .into();

        let new_frontier = Frontier {
            hash: receive.hash(),
            representative: old_frontier.representative,
            balance: new_balance,
            height: old_frontier.height + 1,
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);

        SavedBlock::new(receive, sideband)
    }

    pub fn legacy_change(&mut self, new_representative: impl Into<PublicKey>) -> SavedBlock {
        let old_frontier = self.get_frontier();
        let new_representative = new_representative.into();
        let work = self.create_pow(old_frontier.hash);

        let change: Block = ChangeBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            representative: new_representative,
            work,
        }
        .into();

        let new_frontier = Frontier {
            hash: change.hash(),
            representative: new_representative,
            height: old_frontier.height + 1,
            ..old_frontier
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);

        self.set_new_frontier(new_frontier);

        SavedBlock::new(change, sideband)
    }

    pub fn change(&mut self, new_representative: impl Into<PublicKey>) -> SavedBlock {
        let old_frontier = self.get_frontier();
        let new_representative = new_representative.into();
        let change: Block = StateBlockArgs {
            key: self.key,
            previous: old_frontier.hash,
            representative: new_representative,
            balance: old_frontier.balance,
            link: Link::ZERO,
            work: self.create_pow(old_frontier.hash),
        }
        .into();

        let new_frontier = Frontier {
            hash: change.hash(),
            representative: new_representative,
            balance: old_frontier.balance,
            height: old_frontier.height + 1,
        };
        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);
        self.set_new_frontier(new_frontier);

        SavedBlock::new(change, sideband)
    }

    pub fn epoch1(&mut self) -> SavedBlock {
        let old_frontier = self.get_frontier();
        let epoch: Block = EpochBlockArgs {
            epoch_signer: dev_epoch1_signer(),
            account: self.key.account(),
            previous: old_frontier.hash,
            representative: old_frontier.representative,
            balance: old_frontier.balance,
            link: epoch_v1_link(),
            work: self.create_pow(old_frontier.hash),
        }
        .into();

        let new_frontier = Frontier {
            hash: epoch.hash(),
            height: old_frontier.height + 1,
            ..old_frontier
        };

        let sideband = self
            .lattice
            .sideband_for(self.key.account(), &old_frontier, &new_frontier);
        self.set_new_frontier(new_frontier);

        SavedBlock::new(epoch, sideband)
    }

    fn set_new_frontier(&mut self, new_frontier: Frontier) {
        self.lattice
            .accounts
            .insert(self.key.account(), new_frontier);
    }

    fn get_frontier(&self) -> Frontier {
        self.lattice
            .accounts
            .get(&self.key.account())
            .expect("Cannot send/change from unopenend account!")
            .clone()
    }

    fn get_frontier_or_empty(&self) -> Frontier {
        self.lattice
            .accounts
            .get(&self.key.account())
            .cloned()
            .unwrap_or_else(|| Frontier {
                hash: BlockHash::ZERO,
                representative: self.key.public_key(),
                balance: Amount::ZERO,
                height: 0,
            })
    }
}

/// Deterministic, single-threaded PoW generator for tests so ledger users
/// avoid the heavy `rsnano_work` dependency.
#[derive(Clone)]
struct WorkGenerator {
    mode: WorkGeneratorMode,
}

#[derive(Clone)]
enum WorkGeneratorMode {
    Deterministic(WorkThresholds),
    Stub(WorkNonce),
}

impl WorkGenerator {
    fn dev_network() -> Self {
        Self {
            mode: WorkGeneratorMode::Deterministic(WorkThresholds::publish_dev().clone()),
        }
    }

    fn stub(work: WorkNonce) -> Self {
        Self {
            mode: WorkGeneratorMode::Stub(work),
        }
    }

    fn generate(&self, root: Root) -> WorkNonce {
        match &self.mode {
            WorkGeneratorMode::Stub(work) => *work,
            WorkGeneratorMode::Deterministic(thresholds) => {
                generate_pow(thresholds, root, dev_difficulty())
            }
        }
    }
}

fn generate_pow(thresholds: &WorkThresholds, root: Root, required: u64) -> WorkNonce {
    let mut candidate = 0u64;
    loop {
        let work = WorkNonce::new(candidate);
        if thresholds.difficulty(&root, work) >= required {
            return work;
        }
        candidate = candidate.wrapping_add(1);
    }
}
