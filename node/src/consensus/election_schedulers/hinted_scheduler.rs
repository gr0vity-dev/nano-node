use std::{
    cmp::min,
    collections::{BTreeMap, HashMap, HashSet},
    mem::size_of,
    sync::{Arc, Mutex},
    time::Duration,
};

use rsnano_ledger::{AnySet, Ledger, LedgerSet};
use rsnano_nullable_clock::{SteadyClock, Timestamp};
use rsnano_types::{Amount, BlockHash};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats},
};

use super::VoteCache;
use crate::{
    cementation::ConfirmingSet,
    consensus::{AecInsertRequest, AecService, election::ElectionBehavior},
    representatives::OnlineReps,
};

#[derive(Clone, Debug, PartialEq)]
pub struct HintedSchedulerConfig {
    pub check_interval: Duration,
    pub block_cooldown: Duration,
    pub hinting_threshold_percent: u32,
    pub vacancy_threshold_percent: u32,
    /// Limit of hinted elections as percentage of `active_elections_size`
    pub hinted_limit_percentage: usize,
}

impl HintedSchedulerConfig {
    pub fn default_for_dev_network() -> Self {
        Self {
            check_interval: Duration::from_millis(100),
            block_cooldown: Duration::from_millis(100),
            ..Default::default()
        }
    }
}

impl Default for HintedSchedulerConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_millis(1000),
            block_cooldown: Duration::from_millis(5000),
            hinting_threshold_percent: 10,
            vacancy_threshold_percent: 20,
            hinted_limit_percentage: 20,
        }
    }
}

/// Monitors inactive vote cache and schedules elections with the highest observed vote tally.
pub struct HintedScheduler {
    logic: HintedSchedulerLogic,
    state: Mutex<HintedSchedulerState>,
    active_elections: Arc<AecService>,
    ledger: Arc<Ledger>,
    confirming_set: Arc<ConfirmingSet>,
    stats: Arc<Stats>,
    vote_cache: Arc<Mutex<VoteCache>>,
    online_reps: Arc<Mutex<OnlineReps>>,
    clock: Arc<SteadyClock>,
    pub max_elections: usize,
}

impl HintedScheduler {
    pub fn new(
        config: HintedSchedulerConfig,
        active_elections: Arc<AecService>,
        ledger: Arc<Ledger>,
        stats: Arc<Stats>,
        vote_cache: Arc<Mutex<VoteCache>>,
        confirming_set: Arc<ConfirmingSet>,
        online_reps: Arc<Mutex<OnlineReps>>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let max_elections = active_elections.max_len() * config.hinted_limit_percentage / 100;

        Self {
            logic: HintedSchedulerLogic::new(config, max_elections),
            state: Mutex::new(HintedSchedulerState::default()),
            active_elections,
            ledger,
            stats,
            vote_cache,
            confirming_set,
            online_reps,
            clock,
            max_elections,
        }
    }

    fn hinted_vacancy(&self) -> i64 {
        self.logic.hinted_vacancy(
            self.active_elections
                .count_by_behavior(ElectionBehavior::Hinted),
            self.active_elections.vacancy(),
        )
    }

    pub fn container_info(&self) -> ContainerInfo {
        let guard = self.state.lock().unwrap();
        [(
            "cooldowns",
            guard.cooldowns.len(),
            (size_of::<BlockHash>() + size_of::<Timestamp>()) * 2,
        )]
        .into()
    }

    pub fn check_interval(&self) -> Duration {
        self.logic.check_interval()
    }

    fn predicate(&self) -> bool {
        // Check if there is space inside AEC for a new hinted election
        self.hinted_vacancy() > 0
    }

    fn activate(&self, any: &impl AnySet, hash: BlockHash, check_dependents: bool) -> bool {
        const MAX_ITERATIONS: usize = 64;
        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        stack.push(hash);
        let mut iterations = 0;
        let mut inserted_any = false;
        while let Some(current_hash) = stack.pop() {
            if iterations >= MAX_ITERATIONS {
                break;
            }
            iterations += 1;

            // Check if block exists
            if let Some(block) = any.get_block(&current_hash) {
                let forked = {
                    #[cfg(not(feature = "ledger_snapshots"))]
                    {
                        false
                    }
                    #[cfg(feature = "ledger_snapshots")]
                    {
                        any.is_forked(&block.qualified_root())
                    }
                };

                // Ensure block is not already confirmed
                let is_confirmed = self.confirming_set.contains(&current_hash)
                    || any.confirmed().block_exists(&current_hash);

                if is_confirmed && !forked {
                    self.stats
                        .inc(StatType::Hinting, DetailType::AlreadyConfirmed);
                    self.vote_cache.lock().unwrap().erase(&current_hash); // Remove from vote cache
                    continue; // Move on to the next item in the stack
                }

                if check_dependents {
                    // Perform a depth-first search of the dependency graph
                    if !any.dependencies_confirmed(&block) {
                        self.stats
                            .inc(StatType::Hinting, DetailType::DependentUnconfirmed);
                        let dependents = any.block_dependencies(&block);
                        for dependent_hash in dependents.iter() {
                            // Avoid visiting the same block twice
                            if !dependent_hash.is_zero() && visited.insert(*dependent_hash) {
                                stack.push(*dependent_hash); // Add dependent block to the stack
                            }
                        }
                        continue; // Move on to the next item in the stack
                    }
                }

                // Try to insert it into AEC as hinted election
                let now = self.clock.now();
                let priority = any.block_priority(&block);
                let inserted = self
                    .active_elections
                    .insert(AecInsertRequest::new_hinted(block, priority), now)
                    .is_ok();

                self.stats.inc(
                    StatType::Hinting,
                    if inserted {
                        DetailType::Insert
                    } else {
                        DetailType::InsertFailed
                    },
                );
                inserted_any |= inserted;
            } else {
                self.stats.inc(StatType::Hinting, DetailType::MissingBlock);

                // TODO: Block is missing, bootstrap it
            }
        }
        inserted_any
    }

    fn run_interactive(&self) -> bool {
        let minimum_tally = self.logic.tally_threshold(
            self.online_reps
                .lock()
                .unwrap()
                .trended_or_minimum_weight(),
        );
        let minimum_final_tally = self.online_reps.lock().unwrap().quorum_delta();

        // Get the list before db transaction starts to avoid unnecessary slowdowns
        let tops = self.vote_cache.lock().unwrap().top(minimum_tally);

        let mut any = self.ledger.any();
        let mut inserted_any = false;
        let mut state = self.state.lock().unwrap();

        for entry in tops {
            if !self.predicate() {
                return inserted_any;
            }

            if self
                .logic
                .cooldown(&mut state, entry.hash, self.clock.now())
            {
                continue;
            }

            if any.should_refresh() {
                any = self.ledger.any();
            }

            // Check dependents only if cached tally is lower than quorum
            if self
                .logic
                .should_check_dependents(entry.final_tally, minimum_final_tally)
            {
                // Ensure all dependent blocks are already confirmed before activating
                self.stats.inc(StatType::Hinting, DetailType::Activate);
                inserted_any |=
                    self.activate(&any, entry.hash, /* activate dependents */ true);
            } else {
                // Blocks with a vote tally higher than quorum, can be activated and confirmed immediately
                self.stats
                    .inc(StatType::Hinting, DetailType::ActivateImmediate);
                inserted_any |= self.activate(&any, entry.hash, false);
            }
        }
        inserted_any
    }

    pub fn run_one(&self) -> bool {
        self.stats.inc(StatType::Hinting, DetailType::Loop);
        if !self.predicate() {
            return false;
        }
        self.run_interactive()
    }
}

#[derive(Default)]
struct HintedSchedulerState {
    cooldowns: OrderedCooldowns,
}

struct HintedSchedulerLogic {
    check_interval: Duration,
    block_cooldown: Duration,
    hinting_threshold_percent: u32,
    max_elections: usize,
}

impl HintedSchedulerLogic {
    fn new(config: HintedSchedulerConfig, max_elections: usize) -> Self {
        Self {
            check_interval: config.check_interval,
            block_cooldown: config.block_cooldown,
            hinting_threshold_percent: config.hinting_threshold_percent,
            max_elections,
        }
    }

    fn check_interval(&self) -> Duration {
        self.check_interval
    }

    fn hinted_vacancy(&self, hinted_count: usize, aec_vacancy: i64) -> i64 {
        min(self.max_elections as i64 - hinted_count as i64, aec_vacancy)
    }

    fn tally_threshold(&self, online_weight: Amount) -> Amount {
        (online_weight / 100) * self.hinting_threshold_percent as u128
    }

    fn should_check_dependents(&self, final_tally: Amount, final_tally_threshold: Amount) -> bool {
        final_tally < final_tally_threshold
    }

    fn cooldown(
        &self,
        state: &mut HintedSchedulerState,
        hash: BlockHash,
        now: Timestamp,
    ) -> bool {
        let cooldowns = &mut state.cooldowns;
        if let Some(timeout) = cooldowns.get(&hash) {
            if *timeout > now {
                return true;
            }
            cooldowns.remove(&hash);
        }

        cooldowns.insert(hash, now + self.block_cooldown);
        cooldowns.trim(now);
        false
    }
}

#[derive(Default)]
struct OrderedCooldowns {
    by_hash: HashMap<BlockHash, Timestamp>,
    by_time: BTreeMap<Timestamp, Vec<BlockHash>>,
}

impl OrderedCooldowns {
    fn insert(&mut self, hash: BlockHash, timeout: Timestamp) {
        if let Some(old_timeout) = self.by_hash.insert(hash, timeout) {
            self.remove_timeout_entry(&hash, old_timeout);
        }
        self.by_time.entry(timeout).or_default().push(hash);
    }

    fn get(&self, hash: &BlockHash) -> Option<&Timestamp> {
        self.by_hash.get(hash)
    }

    fn remove(&mut self, hash: &BlockHash) {
        if let Some(timeout) = self.by_hash.remove(hash) {
            self.remove_timeout_entry(hash, timeout);
        }
    }

    fn remove_timeout_entry(&mut self, hash: &BlockHash, timeout: Timestamp) {
        if let Some(hashes) = self.by_time.get_mut(&timeout) {
            if hashes.len() == 1 {
                self.by_time.remove(&timeout);
            } else {
                hashes.retain(|h| h != hash)
            }
        }
    }

    fn trim(&mut self, now: Timestamp) {
        while let Some(entry) = self.by_time.first_entry() {
            if *entry.key() <= now {
                let hashes = entry.remove();
                for hash in hashes {
                    self.by_hash.remove(&hash);
                }
            } else {
                break;
            }
        }
    }

    fn len(&self) -> usize {
        self.by_hash.len()
    }
}
