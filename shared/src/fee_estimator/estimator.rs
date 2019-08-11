use super::tx_confirm_stat::TxConfirmStat;
use crate::fee_rate::FeeRate;
use numext_fixed_hash::H256;
use std::collections::HashMap;

pub const MAX_CONFIRM_BLOCKS: usize = 1000;
const MIN_BUCKET_FEERATE: f64 = 1000f64;
const MAX_BUCKET_FEERATE: f64 = 1e7;
const FEE_SPACING: f64 = 1.05f64;
const MIN_ESTIMATE_SAMPLES: usize = 20;
const MIN_ESTIMATE_CONFIRM_RATE: f64 = 0.85f64;
/// half life each 100 blocks, math.exp(math.log(0.5) / 100)
const DEFAULT_DECAY_FACTOR: f64 = 0.993;

#[derive(Clone)]
struct TxRecord {
    height: u64,
    bucket_index: usize,
    fee_rate: FeeRate,
}

/// Fee Estimator
/// Estimator track new_block and tx_pool to collect data
/// we track every new tx enter txpool and record the tip height and fee_rate,
/// when tx is packed into a new block or dropped by txpool we get a sample about how long a tx with X fee_rate can get confirmed or get dropped.
///
/// In inner, we group samples by predefined fee_rate ranges and store them into buckets.
/// To estimator fee_rate, we travel through these buckets, and try find a proper fee_rate X to let a tx get confirm with Y propolity within T blocks.
///
#[derive(Clone)]
pub struct Estimator {
    best_height: u64,
    start_height: u64,
    tx_confirm_stat: TxConfirmStat,
    tracked_txs: HashMap<H256, TxRecord>,
}

impl Default for Estimator {
    fn default() -> Self {
        Self::new()
    }
}

impl Estimator {
    pub fn new() -> Self {
        let mut buckets = Vec::new();
        let mut bucket_fee_boundary = MIN_BUCKET_FEERATE;
        while bucket_fee_boundary <= MAX_BUCKET_FEERATE {
            buckets.push(FeeRate::from_u64(bucket_fee_boundary as u64));
            bucket_fee_boundary *= FEE_SPACING;
        }
        Estimator {
            best_height: 0,
            start_height: 0,
            tx_confirm_stat: TxConfirmStat::new(&buckets, MAX_CONFIRM_BLOCKS, DEFAULT_DECAY_FACTOR),
            tracked_txs: Default::default(),
        }
    }

    fn process_block_tx(&mut self, height: u64, tx_hash: &H256) -> bool {
        if let Some(tx) = self.drop_tx_inner(tx_hash, false) {
            let blocks_to_confirm = height.saturating_sub(tx.height) as usize;
            self.tx_confirm_stat
                .add_confirmed_tx(blocks_to_confirm, tx.fee_rate);
            true
        } else {
            // tx is not tracked
            false
        }
    }

    /// process new block
    pub fn process_block<'a>(&mut self, height: u64, txs: impl Iterator<Item = &'a H256>) {
        // For simpfy, we assume chain reorg will not effect tx fee.
        if height <= self.best_height {
            return;
        }
        self.best_height = height;
        self.tx_confirm_stat.move_track_window(height);
        self.tx_confirm_stat.decay();
        let processed_txs = txs.filter(|tx| self.process_block_tx(height, tx)).count();
        if self.start_height == 0 && processed_txs > 0 {
            // start record
            self.start_height = self.best_height;
        }
    }
    /// new enter pool tx
    pub fn track_tx(&mut self, tx_hash: H256, fee_rate: FeeRate, height: u64) {
        if self.tracked_txs.contains_key(&tx_hash) {
            // already in track
            return;
        }
        if height != self.best_height {
            // ignore wrong height txs
            return;
        }
        if let Some(bucket_index) = self.tx_confirm_stat.add_unconfirmed_tx(height, fee_rate) {
            self.tracked_txs.insert(
                tx_hash,
                TxRecord {
                    height,
                    bucket_index,
                    fee_rate,
                },
            );
        }
    }

    fn drop_tx_inner(&mut self, tx_hash: &H256, count_failure: bool) -> Option<TxRecord> {
        self.tracked_txs.remove(tx_hash).map(|tx_record| {
            self.tx_confirm_stat.remove_unconfirmed_tx(
                tx_record.height,
                self.best_height,
                tx_record.bucket_index,
                count_failure,
            );
            tx_record
        })
    }
    /// tx removed by txpool
    pub fn drop_tx(&mut self, tx_hash: &H256) -> bool {
        self.drop_tx_inner(tx_hash, true).is_some()
    }

    pub fn estimate(&self, confirm_target: usize) -> FeeRate {
        self.tx_confirm_stat.estimate_median(
            confirm_target,
            MIN_ESTIMATE_SAMPLES,
            MIN_ESTIMATE_CONFIRM_RATE,
        )
    }
}
