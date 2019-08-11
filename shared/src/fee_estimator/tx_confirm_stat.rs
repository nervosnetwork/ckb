use crate::fee_rate::FeeRate;
use std::collections::BTreeMap;

#[derive(Default, Debug, Clone)]
struct BucketStat {
    total_fee_rate: FeeRate,
    txs_count: f64,
    old_unconfirmed_txs: usize,
}

impl BucketStat {
    fn new_fee_rate_sample(&mut self, fee_rate: FeeRate) {
        self.txs_count += 1f64;
        self.total_fee_rate = self.total_fee_rate.saturating_add(fee_rate);
    }

    fn avg_fee_rate(&self) -> FeeRate {
        if self.txs_count != 0f64 {
            FeeRate::from_u64((self.total_fee_rate.as_u64() / self.txs_count as u64) as u64)
        } else {
            FeeRate::zero()
        }
    }
}

/// Track tx fee_rate and confirmation time,
/// this struct track unconfirmed txs count when tx added to or remove from txpool,
/// when a tx confirmed, put it into buckets by tx fee_rate,
/// estimate median fee by look up each buckets until meet required_confirm_rate.
#[derive(Clone)]
pub struct TxConfirmStat {
    /// per bucket stat
    bucket_stats: Vec<BucketStat>,
    /// bucket upper bound fee_rate => bucket index
    fee_rate_to_bucket: BTreeMap<FeeRate, usize>,
    /// confirm_blocks => bucket index => confirmed txs count
    confirm_blocks_to_confirmed_txs: Vec<Vec<f64>>,
    /// confirm_blocks => bucket index => failed txs count
    confirm_blocks_to_failed_txs: Vec<Vec<f64>>,
    /// Track recent N blocks unconfirmed txs
    /// tracked block index => bucket index => TxTracker
    block_unconfirmed_txs: Vec<Vec<usize>>,
    decay_factor: f64,
}

impl TxConfirmStat {
    pub fn new(buckets: &[FeeRate], max_confirm_blocks: usize, decay_factor: f64) -> Self {
        let mut bucket_stats = Vec::with_capacity(buckets.len());
        bucket_stats.resize_with(buckets.len(), BucketStat::default);
        let fee_rate_to_bucket = buckets
            .iter()
            .enumerate()
            .map(|(i, fee_rate)| (*fee_rate, i))
            .collect();
        let mut confirm_blocks_to_confirmed_txs = Vec::with_capacity(max_confirm_blocks);
        confirm_blocks_to_confirmed_txs.resize_with(max_confirm_blocks, Vec::new);
        confirm_blocks_to_confirmed_txs
            .iter_mut()
            .for_each(|bucket| {
                bucket.resize(buckets.len(), 0f64);
            });
        let mut confirm_blocks_to_failed_txs = Vec::with_capacity(max_confirm_blocks);
        confirm_blocks_to_failed_txs.resize_with(max_confirm_blocks, Vec::new);
        confirm_blocks_to_failed_txs.iter_mut().for_each(|bucket| {
            bucket.resize(buckets.len(), 0f64);
        });
        let mut block_unconfirmed_txs = Vec::with_capacity(max_confirm_blocks);
        block_unconfirmed_txs.resize_with(max_confirm_blocks, Vec::new);
        block_unconfirmed_txs.iter_mut().for_each(|bucket| {
            bucket.resize(buckets.len(), 0);
        });
        TxConfirmStat {
            bucket_stats,
            fee_rate_to_bucket,
            block_unconfirmed_txs,
            confirm_blocks_to_confirmed_txs,
            confirm_blocks_to_failed_txs,
            decay_factor,
        }
    }

    /// Return upper bound fee_rate bucket
    /// assume we have three buckets with fee_rate [1.0, 2.0, 3.0], we return index 1 for fee_rate 1.5
    fn bucket_index_by_fee_rate(&self, fee_rate: FeeRate) -> Option<usize> {
        self.fee_rate_to_bucket
            .range(fee_rate..)
            .next()
            .map(|(_fee_rate, index)| *index)
    }

    fn max_confirms(&self) -> usize {
        self.confirm_blocks_to_confirmed_txs.len()
    }

    // add confirmed sample
    pub fn add_confirmed_tx(&mut self, blocks_to_confirm: usize, fee_rate: FeeRate) {
        if blocks_to_confirm < 1 {
            return;
        }
        let bucket_index = match self.bucket_index_by_fee_rate(fee_rate) {
            Some(index) => index,
            None => return,
        };
        // increase txs_count in buckets
        for i in (blocks_to_confirm - 1)..self.max_confirms() {
            self.confirm_blocks_to_confirmed_txs[i][bucket_index] += 1f64;
        }
        let stat = &mut self.bucket_stats[bucket_index];
        stat.new_fee_rate_sample(fee_rate);
    }

    // track an unconfirmed tx
    // entry_height - tip number when tx enter txpool
    pub fn add_unconfirmed_tx(&mut self, entry_height: u64, fee_rate: FeeRate) -> Option<usize> {
        let bucket_index = match self.bucket_index_by_fee_rate(fee_rate) {
            Some(index) => index,
            None => return None,
        };
        let block_index = (entry_height % (self.block_unconfirmed_txs.len() as u64)) as usize;
        self.block_unconfirmed_txs[block_index][bucket_index] += 1;
        Some(bucket_index)
    }

    pub fn remove_unconfirmed_tx(
        &mut self,
        entry_height: u64,
        tip_height: u64,
        bucket_index: usize,
        count_failure: bool,
    ) {
        let tx_age = tip_height.saturating_sub(entry_height) as usize;
        if tx_age < 1 {
            return;
        }
        if tx_age >= self.block_unconfirmed_txs.len() {
            self.bucket_stats[bucket_index].old_unconfirmed_txs -= 1;
        } else {
            let block_index = (entry_height % self.block_unconfirmed_txs.len() as u64) as usize;
            self.block_unconfirmed_txs[block_index][bucket_index] -= 1;
        }
        if count_failure {
            self.confirm_blocks_to_failed_txs[tx_age - 1][bucket_index] += 1f64;
        }
    }

    pub fn move_track_window(&mut self, height: u64) {
        let block_index = (height % (self.block_unconfirmed_txs.len() as u64)) as usize;
        for bucket_index in 0..self.bucket_stats.len() {
            // mark unconfirmed txs as old_unconfirmed_txs
            self.bucket_stats[bucket_index].old_unconfirmed_txs +=
                self.block_unconfirmed_txs[block_index][bucket_index];
            self.block_unconfirmed_txs[block_index][bucket_index] = 0;
        }
    }

    /// apply decay factor on stats
    /// this behavior will smoothly remove the effects from old data, and moving forward to effects from new data.
    pub fn decay(&mut self) {
        let decay_factor = self.decay_factor;
        for (bucket_index, bucket) in self.bucket_stats.iter_mut().enumerate() {
            self.confirm_blocks_to_confirmed_txs
                .iter_mut()
                .for_each(|buckets| {
                    buckets[bucket_index] *= decay_factor;
                });

            self.confirm_blocks_to_failed_txs
                .iter_mut()
                .for_each(|buckets| {
                    buckets[bucket_index] *= decay_factor;
                });
            bucket.total_fee_rate =
                FeeRate::from_u64((bucket.total_fee_rate.as_u64() as f64 * decay_factor) as u64);
            bucket.txs_count *= decay_factor;
            // TODO do we need decay the old unconfirmed?
        }
    }

    /// The naive estimate implementation
    /// 1. find best range of buckets satisfy the given condition
    /// 2. get median fee_rate from best range bucekts
    pub fn estimate_median(
        &self,
        confirm_blocks: usize,
        required_samples: usize,
        required_confirm_rate: f64,
    ) -> FeeRate {
        if confirm_blocks == 0 || required_samples == 0 {
            return FeeRate::zero();
        }
        let mut confirmed_txs = 0f64;
        let mut txs_count = 0f64;
        let mut failure_count = 0f64;
        let mut extra_count = 0;
        let mut best_bucket_start = 0;
        let mut best_bucket_end = 0;
        let mut start_bucket_index = 0;
        let mut find_best = false;
        // try find enough sample data from buckets
        for (bucket_index, stat) in self.bucket_stats.iter().enumerate() {
            confirmed_txs += self.confirm_blocks_to_confirmed_txs[confirm_blocks - 1][bucket_index];
            failure_count += self.confirm_blocks_to_failed_txs[confirm_blocks - 1][bucket_index];
            extra_count += &self.block_unconfirmed_txs[confirm_blocks - 1][bucket_index];
            txs_count += stat.txs_count;
            // we have enough data
            while txs_count as usize >= required_samples {
                let confirm_rate = confirmed_txs / (txs_count + failure_count + extra_count as f64);
                // satisfied required_confirm_rate, find the best buckets range
                if confirm_rate >= required_confirm_rate {
                    best_bucket_start = start_bucket_index;
                    best_bucket_end = bucket_index;
                    find_best = true;
                    break;
                } else {
                    // remove sample data of the first bucket in the range, then retry
                    let stat = self.bucket_stats.get(start_bucket_index).expect("exists");
                    confirmed_txs -= self.confirm_blocks_to_confirmed_txs[confirm_blocks - 1]
                        [start_bucket_index];
                    failure_count -=
                        self.confirm_blocks_to_failed_txs[confirm_blocks - 1][start_bucket_index];
                    extra_count -=
                        &self.block_unconfirmed_txs[confirm_blocks - 1][start_bucket_index];
                    txs_count -= stat.txs_count;
                    start_bucket_index += 1;
                    continue;
                }
            }

            // end loop if we found the best buckets
            if find_best {
                break;
            }
        }

        if !find_best {
            return FeeRate::zero();
        }

        let best_range_txs_count: f64 = self.bucket_stats[best_bucket_start..=best_bucket_end]
            .iter()
            .map(|b| b.txs_count)
            .sum();

        // find median bucket
        if best_range_txs_count != 0f64 {
            let mut half_count = best_range_txs_count / 2f64;
            for bucket in &self.bucket_stats[best_bucket_start..=best_bucket_end] {
                // find the median bucket
                if bucket.txs_count >= half_count {
                    return bucket.avg_fee_rate();
                } else {
                    half_count -= bucket.txs_count;
                }
            }
        }
        FeeRate::zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_median() {
        let mut bucket_fee_rate = 1000;
        let bucket_end_fee_rate = 5000;
        let rate = 1.1f64;
        // decay = exp(ln(0.5) / 100), so decay.pow(100) =~ 0.5
        let decay = 0.993f64;
        let max_confirm_blocks = 1000;
        // prepare fee rate buckets
        let mut buckets = vec![];
        while bucket_fee_rate < bucket_end_fee_rate {
            buckets.push(FeeRate::from_u64(bucket_fee_rate));
            bucket_fee_rate = (rate * bucket_fee_rate as f64) as u64;
        }
        let mut stat = TxConfirmStat::new(&buckets, max_confirm_blocks, decay);
        // txs data
        let fee_rate_and_confirms = vec![
            (2500, 5),
            (3000, 5),
            (3500, 5),
            (1500, 10),
            (2000, 10),
            (2100, 10),
            (2200, 10),
            (1200, 15),
            (1000, 15),
        ];
        for (fee_rate, blocks_to_confirm) in fee_rate_and_confirms {
            stat.add_confirmed_tx(blocks_to_confirm, FeeRate::from_u64(fee_rate));
        }
        // test basic median fee rate
        assert_eq!(stat.estimate_median(5, 3, 1f64), FeeRate::from_u64(3000));
        // test different required samples
        assert_eq!(stat.estimate_median(10, 1, 1f64), FeeRate::from_u64(1500));
        assert_eq!(stat.estimate_median(10, 3, 1f64), FeeRate::from_u64(2050));
        assert_eq!(stat.estimate_median(10, 4, 1f64), FeeRate::from_u64(2050));
        assert_eq!(stat.estimate_median(15, 2, 1f64), FeeRate::from_u64(1000));
        assert_eq!(stat.estimate_median(15, 3, 1f64), FeeRate::from_u64(1200));
        // test return zero if confirm_blocks or required_samples is zero
        assert_eq!(stat.estimate_median(0, 4, 1f64), FeeRate::zero());
        assert_eq!(stat.estimate_median(15, 0, 1f64), FeeRate::zero());
        assert_eq!(stat.estimate_median(0, 3, 1f64), FeeRate::zero());
    }
}
