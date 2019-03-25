use numext_fixed_hash::H256;

pub trait BlockMedianTimeContext {
    fn block_count(&self) -> u32;
    fn timestamp(&self, hash: &H256) -> Option<u64>;
    fn parent_hash(&self, hash: &H256) -> Option<H256>;
    fn block_median_time(&self, hash: &H256) -> Option<u64> {
        let count = self.block_count() as usize;
        let mut block_times = Vec::with_capacity(count);
        let mut current_hash = hash.to_owned();
        for _ in 0..count {
            match self.timestamp(&current_hash) {
                Some(timestamp) => block_times.push(timestamp),
                None => break,
            }
            match self.parent_hash(&current_hash) {
                Some(hash) => current_hash = hash,
                None => break,
            }
        }
        block_times.sort_by(|a, b| b.cmp(a));
        block_times.get(block_times.len() / 2).cloned()
    }
}
