use rsnano_types::{Amount, BlockHash};

use super::{Election, VoteSummary};

/// Counts the tally per block in an election.
/// It is sorted by descending tally
#[derive(Default, Clone)]
pub struct BlockTallies {
    tallies: [(BlockHash, Amount); Election::MAX_BLOCKS],
    len: usize,
}

impl BlockTallies {
    pub(crate) fn new() -> Self {
        Default::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &'_ (BlockHash, Amount)> {
        self.tallies[..self.len()].iter()
    }

    pub fn tallies(&self) -> impl Iterator<Item = Amount> + use<'_> {
        self.iter().map(|(_, tally)| *tally)
    }

    pub fn winner(&self) -> Option<&(BlockHash, Amount)> {
        self.iter().next()
    }

    pub fn lowest(&self) -> Option<&(BlockHash, Amount)> {
        self.iter().last()
    }

    pub fn get(&self, hash: &BlockHash) -> Amount {
        self.iter()
            .find_map(|(h, tally)| if h == hash { Some(*tally) } else { None })
            .unwrap_or_default()
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.iter().any(|(h, _)| h == hash)
    }

    pub fn sum(&self) -> Amount {
        self.tallies().sum()
    }

    pub fn remove(&mut self, hash: &BlockHash) {
        let index = self
            .iter()
            .enumerate()
            .find_map(|(i, (h, _))| if h == hash { Some(i) } else { None });

        if let Some(i) = index {
            self.tallies[i..].rotate_left(1);
            self.len -= 1;
        }
    }

    pub fn add_amount(&mut self, hash: BlockHash, amount: Amount) {
        if let Some((_, tally)) = self.tallies[..self.len]
            .iter_mut()
            .find(|(existing, _)| *existing == hash)
        {
            *tally += amount;
        } else {
            self.insert_unsorted(hash, amount);
        }
        self.sort_by_descending_tally();
    }

    pub fn subtract_amount(&mut self, hash: &BlockHash, amount: Amount) {
        let Some(index) = self
            .iter()
            .enumerate()
            .find_map(|(i, (existing, _))| if existing == hash { Some(i) } else { None })
        else {
            return;
        };

        let tally = &mut self.tallies[index].1;
        *tally -= amount;
        if tally.is_zero() {
            self.tallies[index..].rotate_left(1);
            self.len -= 1;
        } else {
            self.sort_by_descending_tally();
        }
    }

    pub fn check_quorum(&self, quorum_delta: Amount) -> bool {
        let mut it = self.tallies();
        let first = it.next().unwrap_or_default();
        let second = it.next().unwrap_or_default();
        first - second >= quorum_delta
    }

    pub fn calculate<'a, 'b>(&'a mut self, votes: impl IntoIterator<Item = &'b VoteSummary>) {
        self.len = 0;

        for vote in votes.into_iter() {
            if let Some((_, tally)) = self.tallies[..self.len]
                .iter_mut()
                .find(|(hash, _)| *hash == vote.hash)
            {
                *tally += vote.weight;
            } else {
                self.insert_unsorted(vote.hash, vote.weight);
            }
        }

        self.sort_by_descending_tally();
    }

    pub fn insert(&mut self, hash: BlockHash, tally: Amount) {
        self.insert_unsorted(hash, tally);
        self.sort_by_descending_tally();
    }

    fn insert_unsorted(&mut self, hash: BlockHash, tally: Amount) {
        if self.len == Election::MAX_BLOCKS {
            panic!(
                "Tallies can only be counted for {} blocks",
                Election::MAX_BLOCKS
            );
        }
        self.tallies[self.len] = (hash, tally);
        self.len += 1;
    }

    fn sort_by_descending_tally(&mut self) {
        self.tallies[..self.len].sort_by(|(_, left), (_, right)| right.cmp(left));
    }

    pub fn retain_non_zero(&mut self) {
        let mut next = 0;
        for i in 0..self.len {
            if !self.tallies[i].1.is_zero() {
                self.tallies[next] = self.tallies[i];
                next += 1;
            }
        }
        self.len = next;
    }
}
