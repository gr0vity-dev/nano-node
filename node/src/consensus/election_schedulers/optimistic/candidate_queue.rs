use std::collections::{BTreeMap, HashMap};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::Account;

#[derive(Default)]
pub(super) struct CandidateQueue {
    /// account => gap
    by_account: HashMap<Account, u64>,
    /// gap => (account, insertion timestamp)
    by_gap: BTreeMap<u64, Vec<(Account, Timestamp)>>,
}

impl CandidateQueue {
    pub fn insert(&mut self, account: Account, now: Timestamp, gap: u64) {
        if let Some(old_gap) = self.by_account.insert(account, gap) {
            // Move to new gap bucket, preserving the original insertion timestamp
            if let Some(v) = self.by_gap.get_mut(&old_gap) {
                if let Some(pos) = v.iter().position(|(a, _)| *a == account) {
                    let (_, original_timestamp) = v.remove(pos);
                    if v.is_empty() {
                        self.by_gap.remove(&old_gap);
                    }
                    self.by_gap
                        .entry(gap)
                        .or_default()
                        .push((account, original_timestamp));
                    return;
                }
            }
        }
        self.by_gap.entry(gap).or_default().push((account, now));
    }

    pub fn len(&self) -> usize {
        self.by_account.len()
    }

    pub fn contains(&self, account: &Account) -> bool {
        self.by_account.contains_key(account)
    }

    pub fn min_gap(&self) -> Option<u64> {
        self.by_gap.keys().next().copied()
    }

    pub fn oldest_timestamp(&self) -> Option<Timestamp> {
        self.by_gap
            .values()
            .flat_map(|entries| entries.iter().map(|(_, inserted)| *inserted))
            .min()
    }

    pub fn pop_lowest_gap_entry(&mut self) -> Option<Account> {
        let gap = *self.by_gap.keys().next()?;
        let v = self.by_gap.get_mut(&gap).unwrap();
        let (account, _) = v.pop().unwrap();
        if v.is_empty() {
            self.by_gap.remove(&gap);
        }
        self.by_account.remove(&account);
        Some(account)
    }

    pub fn pop_first(&mut self, cutoff: Timestamp) -> Option<Account> {
        let mut to_remove: Option<u64> = None;
        let mut result = None;
        for (gap, v) in self.by_gap.iter_mut().rev() {
            if let Some((account, _)) = v.pop_if(|(_, inserted)| *inserted <= cutoff) {
                if v.is_empty() {
                    to_remove = Some(*gap);
                }
                result = Some(account);
                break;
            }
        }

        if let Some(gap) = to_remove {
            self.by_gap.remove(&gap);
        }
        if let Some(account) = &result {
            self.by_account.remove(account);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty() {
        let mut q = CandidateQueue::default();
        assert_eq!(q.len(), 0);
        assert!(q.pop_first(now()).is_none());
    }

    #[test]
    fn insert() {
        let mut q = CandidateQueue::default();
        q.insert(Account::from(1), now(), 1);
        assert_eq!(q.len(), 1);
        assert_eq!(q.by_account.len(), 1);
        assert_eq!(q.by_gap.len(), 1);
    }

    #[test]
    fn contains() {
        let mut q = CandidateQueue::default();
        let account = Account::from(1);

        assert!(!q.contains(&account));

        q.insert(account, now(), 1);
        assert!(q.contains(&account));
    }

    #[test]
    fn pop_first_returns_highest_gap_first() {
        let mut q = CandidateQueue::default();
        let a = Account::from(1);
        let b = Account::from(2);
        q.insert(a, now(), 2);
        q.insert(b, now(), 1);
        let account = q.pop_first(now()).unwrap();
        assert_eq!(account, a);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn pop_first_orders_by_gap_descending() {
        let mut q = CandidateQueue::default();
        let a = Account::from(1);
        let b = Account::from(2);
        q.insert(a, now(), 2);
        q.insert(b, now(), 1);

        let first = q.pop_first(now()).unwrap();
        let second = q.pop_first(now()).unwrap();
        assert_eq!(first, a);
        assert_eq!(second, b);
    }

    #[test]
    fn insert_duplicate_updates_gap() {
        let mut q = CandidateQueue::default();
        let a = Account::from(1);
        let b = Account::from(2);
        q.insert(a, now(), 1); // a starts with low gap
        q.insert(b, now(), 2);
        q.insert(a, now(), 3); // re-insert a with higher gap

        assert_eq!(q.len(), 2);
        let first = q.pop_first(now()).unwrap();
        assert_eq!(first, a); // a now has the highest gap
        let second = q.pop_first(now()).unwrap();
        assert_eq!(second, b);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn pop_first_returns_none_when_entry_too_new() {
        let mut q = CandidateQueue::default();
        let future = now() + Duration::from_secs(10);
        q.insert(Account::from(1), future, 1);
        assert!(q.pop_first(now()).is_none());
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn pop_first_skips_highest_gap_if_too_new_falls_back_to_lower_gap() {
        let mut q = CandidateQueue::default();
        let a = Account::from(1);
        let b = Account::from(2);
        let future = now() + Duration::from_secs(10);
        q.insert(a, future, 2); // high gap but too new
        q.insert(b, now(), 1); // low gap but old enough

        let result = q.pop_first(now()).unwrap();
        assert_eq!(result, b);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn insert_duplicate_preserves_original_timestamp() {
        // Regression: updating the gap on a duplicate must not reset the insertion
        // timestamp, or the entry will never age past `activation_delay`.
        let mut q = CandidateQueue::default();
        let account = Account::from(1);
        q.insert(account, now(), 1);

        // Re-insert with a higher gap at a future time
        let future = now() + Duration::from_secs(10);
        q.insert(account, future, 2);

        // Entry should still be eligible at the original cutoff (now), not future
        assert!(q.pop_first(now()).is_some());
    }

    #[test]
    fn contains_returns_false_after_pop() {
        let mut q = CandidateQueue::default();
        let account = Account::from(1);
        q.insert(account, now(), 1);
        q.pop_first(now());
        assert!(!q.contains(&account));
    }

    #[test]
    fn oldest_timestamp_is_none_when_empty() {
        let q = CandidateQueue::default();
        assert_eq!(q.oldest_timestamp(), None);
    }

    #[test]
    fn oldest_timestamp_returns_earliest_entry() {
        let mut q = CandidateQueue::default();
        let now = now();
        let older = now - Duration::from_secs(5);
        q.insert(Account::from(1), now, 1);
        q.insert(Account::from(2), older, 2);

        assert_eq!(q.oldest_timestamp(), Some(older));
    }

    fn now() -> Timestamp {
        Timestamp::new_test_instance()
    }
}
