//! Weight-Units Flow Fee Estimator
//!
//! ### Summary
//!
//! This algorithm is migrated from a Bitcoin fee estimates algorithm.
//!
//! The original algorithm could be found in <https://bitcoiner.live>.
//!
//! ### Details
//!
//! #### Inputs
//!
//! The mempool is categorized into "fee buckets".
//! A bucket represents data about all transactions with a fee greater than or
//! equal to some amount (in `weight`).
//!
//! Each bucket contains 2 numeric values:
//!
//! - `current_weight`, represents the transactions currently sitting in the
//!   mempool.
//!
//! - `flow`, represents the speed at which new transactions are entering the
//!   mempool.
//!
//!   It's sampled by observing the flow of transactions during twice the blocks
//!   count of each target interval (ex: last 60 blocks for the 30 blocks target
//!   interval).
//!
//!   For simplicity, transactions are not looked at individually.
//!   Focus is on the weight, like a fluid flowing from bucket to bucket.
//!
//! #### Computations
//!
//! Let's simulate what's going to happen during each timespan lasting blocks:
//!
//! - New transactions entering the mempool.
//!
//!   While it's impossible to predict sudden changes to the speed at which new
//!   weight is added to the mempool, for simplicty's sake we're going to assume
//!   the flow we measured remains constant: `added_weight = flow * blocks`.
//!
//! - Transactions leaving the mempool due to mined blocks. Each block removes
//!   up to `MAX_BLOCK_BYTES` weight from a bucket.
//!
//!   Once we know the minimum expected number of blocks we can compute how that
//!   would affect the bucket's weight:
//!   `removed_weight = MAX_BLOCK_BYTES * blocks`.
//!
//! - Finally we can compute the expected final weight of the bucket:
//!   `final_weight = current_weight + added_weight - removed_weight`.
//!
//! The cheapest bucket whose `final_weight` is less than or equal to 0 is going
//! to be the one selected as the estimate.

use std::collections::HashMap;

use ckb_chain_spec::consensus::MAX_BLOCK_BYTES;
use ckb_types::core::{
    tx_pool::{get_transaction_weight, TxEntryInfo, TxPoolEntryInfo},
    BlockNumber, BlockView, FeeRate,
};

use crate::{constants, Error};

const FEE_RATE_UNIT: u64 = 1000;

#[derive(Clone)]
pub struct Algorithm {
    boot_tip: BlockNumber,
    current_tip: BlockNumber,
    txs: HashMap<BlockNumber, Vec<TxStatus>>,

    is_ready: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TxStatus {
    weight: u64,
    fee_rate: FeeRate,
}

impl PartialOrd for TxStatus {
    fn partial_cmp(&self, other: &TxStatus) -> Option<::std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TxStatus {
    fn cmp(&self, other: &Self) -> ::std::cmp::Ordering {
        self.fee_rate
            .cmp(&other.fee_rate)
            .then_with(|| other.weight.cmp(&self.weight))
    }
}

impl TxStatus {
    fn new_from_entry_info(info: TxEntryInfo) -> Self {
        let weight = get_transaction_weight(info.size as usize, info.cycles);
        let fee_rate = FeeRate::calculate(info.fee, weight);
        Self { weight, fee_rate }
    }
}

impl Default for Algorithm {
    fn default() -> Self {
        Self::new()
    }
}

impl Algorithm {
    pub fn new() -> Self {
        Self {
            boot_tip: 0,
            current_tip: 0,
            txs: Default::default(),
            is_ready: false,
        }
    }

    pub fn update_ibd_state(&mut self, in_ibd: bool) {
        if self.is_ready {
            if in_ibd {
                self.clear();
                self.is_ready = false;
            }
        } else if !in_ibd {
            self.clear();
            self.is_ready = true;
        }
    }

    fn clear(&mut self) {
        self.boot_tip = 0;
        self.current_tip = 0;
        self.txs.clear();
    }

    pub fn commit_block(&mut self, block: &BlockView) {
        let tip_number = block.number();
        if self.boot_tip == 0 {
            self.boot_tip = tip_number;
        }
        self.current_tip = tip_number;
        self.expire();
    }

    fn expire(&mut self) {
        let historical_blocks = Self::historical_blocks(constants::MAX_TARGET);
        let expired_tip = self.current_tip.saturating_sub(historical_blocks);
        self.txs.retain(|&num, _| num >= expired_tip);
    }

    pub fn accept_tx(&mut self, info: TxEntryInfo) {
        if self.current_tip == 0 {
            return;
        }
        let item = TxStatus::new_from_entry_info(info);
        self.txs
            .entry(self.current_tip)
            .and_modify(|items| items.push(item))
            .or_insert_with(|| vec![item]);
    }

    pub fn estimate_fee_rate(
        &self,
        target_blocks: BlockNumber,
        all_entry_info: TxPoolEntryInfo,
    ) -> Result<FeeRate, Error> {
        if !self.is_ready {
            return Err(Error::NotReady);
        }

        let sorted_current_txs = {
            let mut current_txs: Vec<_> = all_entry_info
                .pending
                .into_values()
                .chain(all_entry_info.proposed.into_values())
                .map(TxStatus::new_from_entry_info)
                .collect();
            current_txs.sort_unstable_by(|a, b| b.cmp(a));
            current_txs
        };

        self.do_estimate(target_blocks, &sorted_current_txs)
    }
}

impl Algorithm {
    fn do_estimate(
        &self,
        target_blocks: BlockNumber,
        sorted_current_txs: &[TxStatus],
    ) -> Result<FeeRate, Error> {
        ckb_logger::debug!(
            "boot: {}, current: {}, target: {target_blocks} blocks",
            self.boot_tip,
            self.current_tip,
        );
        let historical_blocks = Self::historical_blocks(target_blocks);
        ckb_logger::debug!("required: {historical_blocks} blocks");
        if historical_blocks > self.current_tip.saturating_sub(self.boot_tip) {
            return Err(Error::LackData);
        }

        let max_fee_rate = if let Some(fee_rate) = sorted_current_txs.first().map(|tx| tx.fee_rate)
        {
            fee_rate
        } else {
            return Ok(constants::LOWEST_FEE_RATE);
        };

        ckb_logger::debug!("max fee rate of current transactions: {max_fee_rate}");

        let max_bucket_index = Self::max_bucket_index_by_fee_rate(max_fee_rate);
        ckb_logger::debug!("current weight buckets size: {}", max_bucket_index + 1);

        // Create weight buckets.
        let current_weight_buckets = {
            let mut buckets = vec![0u64; max_bucket_index + 1];
            let mut index_curr = max_bucket_index;
            for tx in sorted_current_txs {
                let index = Self::max_bucket_index_by_fee_rate(tx.fee_rate);
                if index < index_curr {
                    let weight_curr = buckets[index_curr];
                    for i in buckets.iter_mut().take(index_curr) {
                        *i = weight_curr;
                    }
                }
                buckets[index] += tx.weight;
                index_curr = index;
            }
            let weight_curr = buckets[index_curr];
            for i in buckets.iter_mut().take(index_curr) {
                *i = weight_curr;
            }
            buckets
        };
        for (index, weight) in current_weight_buckets.iter().enumerate() {
            if *weight != 0 {
                ckb_logger::trace!(">>> current_weight[{index}]: {weight}");
            }
        }

        // Calculate flow speeds for buckets.
        let flow_speed_buckets = {
            let historical_tip = self.current_tip - historical_blocks;
            let sorted_flowed = self.sorted_flowed(historical_tip);
            let mut buckets = vec![0u64; max_bucket_index + 1];
            let mut index_curr = max_bucket_index;
            for tx in &sorted_flowed {
                let index = Self::max_bucket_index_by_fee_rate(tx.fee_rate);
                if index > max_bucket_index {
                    continue;
                }
                if index < index_curr {
                    let flowed_curr = buckets[index_curr];
                    for i in buckets.iter_mut().take(index_curr) {
                        *i = flowed_curr;
                    }
                }
                buckets[index] += tx.weight;
                index_curr = index;
            }
            let flowed_curr = buckets[index_curr];
            for i in buckets.iter_mut().take(index_curr) {
                *i = flowed_curr;
            }
            buckets
                .into_iter()
                .map(|value| value / historical_blocks)
                .collect::<Vec<_>>()
        };
        for (index, speed) in flow_speed_buckets.iter().enumerate() {
            if *speed != 0 {
                ckb_logger::trace!(">>> flow_speed[{index}]: {speed}");
            }
        }

        for bucket_index in 1..=max_bucket_index {
            let current_weight = current_weight_buckets[bucket_index];
            let added_weight = flow_speed_buckets[bucket_index] * target_blocks;
            // Note: blocks are not full even there are many pending transactions,
            // since `MAX_BLOCK_PROPOSALS_LIMIT = 1500`.
            let removed_weight = (MAX_BLOCK_BYTES * 85 / 100) * target_blocks;
            let passed = current_weight + added_weight <= removed_weight;
            ckb_logger::trace!(
                ">>> bucket[{}]: {}; {} + {} - {}",
                bucket_index,
                passed,
                current_weight,
                added_weight,
                removed_weight
            );
            if passed {
                let fee_rate = Self::lowest_fee_rate_by_bucket_index(bucket_index);
                return Ok(fee_rate);
            }
        }

        Err(Error::NoProperFeeRate)
    }

    fn sorted_flowed(&self, historical_tip: BlockNumber) -> Vec<TxStatus> {
        let mut statuses: Vec<_> = self
            .txs
            .iter()
            .filter(|(&num, _)| num >= historical_tip)
            .flat_map(|(_, statuses)| statuses.to_owned())
            .collect();
        statuses.sort_unstable_by(|a, b| b.cmp(a));
        ckb_logger::trace!(">>> sorted flowed length: {}", statuses.len());
        statuses
    }
}

impl Algorithm {
    fn historical_blocks(target_blocks: BlockNumber) -> BlockNumber {
        if target_blocks < constants::MIN_TARGET {
            constants::MIN_TARGET * 2
        } else {
            target_blocks * 2
        }
    }

    fn lowest_fee_rate_by_bucket_index(index: usize) -> FeeRate {
        let t = FEE_RATE_UNIT;
        let value = match index as u64 {
            // 0->0
            0 => 0,
            // 1->1000, 2->2000, .., 10->10000
            x if x <= 10 => t * x,
            // 11->12000, 12->14000, .., 30->50000
            x if x <= 30 => t * (10 + (x - 10) * 2),
            // 31->55000, 32->60000, ..., 60->200000
            x if x <= 60 => t * (10 + 20 * 2 + (x - 30) * 5),
            // 61->210000, 62->220000, ..., 90->500000
            x if x <= 90 => t * (10 + 20 * 2 + 30 * 5 + (x - 60) * 10),
            // 91->520000, 92->540000, ..., 115 -> 1000000
            x if x <= 115 => t * (10 + 20 * 2 + 30 * 5 + 30 * 10 + (x - 90) * 20),
            // 116->1050000, 117->1100000, ..., 135->2000000
            x if x <= 135 => t * (10 + 20 * 2 + 30 * 5 + 30 * 10 + 25 * 20 + (x - 115) * 50),
            // 136->2100000,  137->2200000, ...
            x => t * (10 + 20 * 2 + 30 * 5 + 30 * 10 + 25 * 20 + 20 * 50 + (x - 135) * 100),
        };
        FeeRate::from_u64(value)
    }

    fn max_bucket_index_by_fee_rate(fee_rate: FeeRate) -> usize {
        let t = FEE_RATE_UNIT;
        let index = match fee_rate.as_u64() {
            x if x <= 10_000 => x / t,
            x if x <= 50_000 => (x + t * 10) / (2 * t),
            x if x <= 200_000 => (x + t * 100) / (5 * t),
            x if x <= 500_000 => (x + t * 400) / (10 * t),
            x if x <= 1_000_000 => (x + t * 1_300) / (20 * t),
            x if x <= 2_000_000 => (x + t * 4_750) / (50 * t),
            x => (x + t * 11_500) / (100 * t),
        };
        index as usize
    }
}

#[cfg(test)]
mod tests {
    use super::Algorithm;
    use ckb_types::core::FeeRate;

    #[test]
    fn test_bucket_index_and_fee_rate_expected() {
        let testdata = [
            (0, 0),
            (1, 1_000),
            (2, 2_000),
            (10, 10_000),
            (11, 12_000),
            (12, 14_000),
            (30, 50_000),
            (31, 55_000),
            (32, 60_000),
            (60, 200_000),
            (61, 210_000),
            (62, 220_000),
            (90, 500_000),
            (91, 520_000),
            (92, 540_000),
            (115, 1_000_000),
            (116, 1_050_000),
            (117, 1_100_000),
            (135, 2_000_000),
            (136, 2_100_000),
            (137, 2_200_000),
        ];
        for (bucket_index, fee_rate) in &testdata[..] {
            let expected_fee_rate =
                Algorithm::lowest_fee_rate_by_bucket_index(*bucket_index).as_u64();
            assert_eq!(expected_fee_rate, *fee_rate);
            let actual_bucket_index =
                Algorithm::max_bucket_index_by_fee_rate(FeeRate::from_u64(*fee_rate));
            assert_eq!(actual_bucket_index, *bucket_index);
        }
    }

    #[test]
    fn test_bucket_index_and_fee_rate_continuous() {
        for fee_rate in 0..3_000_000 {
            let bucket_index = Algorithm::max_bucket_index_by_fee_rate(FeeRate::from_u64(fee_rate));
            let fee_rate_le = Algorithm::lowest_fee_rate_by_bucket_index(bucket_index).as_u64();
            let fee_rate_gt = Algorithm::lowest_fee_rate_by_bucket_index(bucket_index + 1).as_u64();
            assert!(
                fee_rate_le <= fee_rate && fee_rate < fee_rate_gt,
                "Error for bucket[{}]: {} <= {} < {}",
                bucket_index,
                fee_rate_le,
                fee_rate,
                fee_rate_gt,
            );
        }
    }
}
