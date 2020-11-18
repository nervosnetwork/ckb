use crate::tx_confirm_stat::TxConfirmStat;
use crate::FeeRate;
use ckb_logger::debug;
use ckb_types::packed::Byte32;
use std::collections::HashMap;

/// TODO(doc): @doitian
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
/// Estimator track new block and tx_pool to collect data
/// we track every new tx enter txpool and record the tip height and fee_rate,
/// when tx is packed into a new block or dropped by txpool,
/// we get a sample about how long a tx with X fee_rate can get confirmed or get dropped.
///
/// In inner, we group samples by predefined fee_rate buckets.
/// To estimator fee_rate for a confirm target(how many blocks that a tx can get committed),
/// we travel through fee_rate buckets, try to find a fee_rate X to let a tx get committed
/// with high probilities within confirm target blocks.
///
#[derive(Clone)]
pub struct Estimator {
    best_height: u64,
    start_height: u64,
    /// a data struct to track tx confirm status
    tx_confirm_stat: TxConfirmStat,
    tracked_txs: HashMap<Byte32, TxRecord>,
}

impl Default for Estimator {
    fn default() -> Self {
        Self::new()
    }
}

impl Estimator {
    /// TODO(doc): @doitian
    pub fn new() -> Self {
        let mut buckets = Vec::new();
        let mut bucket_fee_boundary = MIN_BUCKET_FEERATE;
        // initialize fee_rate buckets
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

    fn process_block_tx(&mut self, height: u64, tx_hash: &Byte32) -> bool {
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
    /// record confirm blocks for txs which we tracked before.
    pub fn process_block(&mut self, height: u64, txs: impl Iterator<Item = Byte32>) {
        // For simpfy, we assume chain reorg will not effect tx fee.
        if height <= self.best_height {
            return;
        }
        self.best_height = height;
        // update tx confirm stat
        self.tx_confirm_stat.move_track_window(height);
        self.tx_confirm_stat.decay();
        let processed_txs = txs.filter(|tx| self.process_block_tx(height, tx)).count();
        if self.start_height == 0 && processed_txs > 0 {
            // start record
            self.start_height = self.best_height;
            debug!("Fee estimator start recording at {}", self.start_height);
        }
    }

    /// track a tx that entered txpool
    pub fn track_tx(&mut self, tx_hash: Byte32, fee_rate: FeeRate, height: u64) {
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

    fn drop_tx_inner(&mut self, tx_hash: &Byte32, count_failure: bool) -> Option<TxRecord> {
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

    /// tx removed from txpool
    pub fn drop_tx(&mut self, tx_hash: &Byte32) -> bool {
        self.drop_tx_inner(tx_hash, true).is_some()
    }

    /// estimate a fee rate for confirm target
    pub fn estimate(&self, expect_confirm_blocks: usize) -> FeeRate {
        self.tx_confirm_stat.estimate_median(
            expect_confirm_blocks,
            MIN_ESTIMATE_SAMPLES,
            MIN_ESTIMATE_CONFIRM_RATE,
        )
    }
}
