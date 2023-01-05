use crate::{
    error::{Error, Result},
    helper::PrettyDisplay,
    statistics::Statistics,
    types,
    validator::Validator,
};
use ckb_async_runtime::{
    tokio::{
        self,
        sync::{mpsc, oneshot, watch},
    },
    Handle,
};
use ckb_notify::NotifyController;
use ckb_stop_handler::{SignalSender, StopHandler, WATCH_INIT};
use ckb_types::{
    core::{tx_pool::get_transaction_weight, Capacity, FeeRate},
    packed::Byte32,
};
use ckb_util::RwLock;
use faketime::unix_time;
use statrs::distribution::{DiscreteCDF as _, Poisson};
use std::{collections::VecDeque, sync::Arc, time::Duration};

type EstimatorResult = Result<Option<u64>>;
const SUBSCRIBER_NAME: &str = "FeeEstimator";
const NAME: &str = "weight-flow";
const FEE_RATE_UNIT: u64 = 1000;
pub(crate) const MAX_BLOCK_WEIGHT: u64 = 597_000 * 2;
const LIFETIME_MINUTES: u32 = 60 * 24 * 2;
const LOWEST_FEE_RATE: FeeRate = FeeRate::from_u64(1000);
const MAX_TARGET: Duration = Duration::from_secs(60 * 24 * 60);

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct TxStatus {
    pub(crate) weight: u64,
    pub(crate) fee_rate: FeeRate,
}

#[derive(Debug, Clone)]
pub(crate) struct TxAdded {
    pub(crate) hash: Byte32,
    pub(crate) status: TxStatus,
    pub(crate) added_dt: Duration,
}

struct TxAddedQue(VecDeque<TxAdded>);

/// Weight-Units Flow Fee Estimator
///
/// Ref: https://bitcoiner.live/?tab=info
pub struct FeeEstimator {
    lowest_fee_rate: FeeRate,
    max_target_dur: Duration,
    boot_dt: Duration,
    txs: TxAddedQue,
    validator: Validator<Duration>,
    statistics: Arc<RwLock<Statistics>>,
}

impl TxStatus {
    pub(crate) fn new(weight: u64, fee_rate: FeeRate) -> Self {
        Self { weight, fee_rate }
    }
}

impl PartialOrd for TxStatus {
    fn partial_cmp(&self, other: &TxStatus) -> Option<::std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TxStatus {
    fn cmp(&self, other: &Self) -> ::std::cmp::Ordering {
        let order = self.fee_rate.cmp(&other.fee_rate);
        if order == ::std::cmp::Ordering::Equal {
            other.weight.cmp(&self.weight)
        } else {
            order
        }
    }
}

impl TxAdded {
    pub(crate) fn new(hash: Byte32, weight: u64, fee_rate: FeeRate, added_dt: Duration) -> Self {
        Self {
            hash,
            status: TxStatus::new(weight, fee_rate),
            added_dt,
        }
    }
}

impl TxAddedQue {
    fn add_transaction(&mut self, tx: TxAdded) {
        self.0.push_front(tx);
    }

    fn remove_transaction(&mut self, tx_hash: &Byte32) {
        self.0.retain(|tx| tx.hash != *tx_hash);
    }

    fn expire(&mut self, expired_dt: Duration) -> usize {
        let count = self
            .0
            .iter()
            .rev()
            .skip_while(|tx| tx.added_dt < expired_dt)
            .count();
        let total = self.0.len();
        if count > 0 && total >= count {
            self.0.truncate(total - count);
        }
        count
    }

    fn flowed(&self, historical_dt: Duration) -> Vec<TxStatus> {
        self.0
            .iter()
            .rev()
            .skip_while(|tx| tx.added_dt >= historical_dt)
            .map(|tx| &tx.status)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    }
}

impl FeeEstimator {
    fn historical_dur(target_dur: Duration) -> Duration {
        if target_dur <= Duration::from_secs(5 * 60) {
            Duration::from_secs(10 * 60)
        } else {
            target_dur * 2
        }
    }

    fn do_estimate(&mut self, probability: f32, target_minutes: u32) -> Option<u64> {
        ckb_logger::trace!(
            "probability: {:.2}%, target: {} mins",
            probability * 100.0,
            target_minutes
        );
        let current_dt = unix_time();
        let target_dur = Duration::from_secs(u64::from(target_minutes) * 60);
        let historical_dur = Self::historical_dur(target_dur);
        ckb_logger::trace!(
            "current timestamp: {}, target: {:?}, historical: {:?}",
            current_dt.pretty(),
            target_dur,
            historical_dur,
        );
        let average_blocks = {
            let filter_blocks_dt = current_dt - historical_dur;
            let blocks_count = self
                .statistics
                .read()
                .filter_blocks(|dt| dt >= filter_blocks_dt, |_, _| Some(()))
                .len() as u32;
            if blocks_count == 0 {
                ckb_logger::warn!("historical: no blocks");
                return None;
            }
            let interval_dur = historical_dur / blocks_count;
            ckb_logger::trace!(
                "historical: blocks count: {} in {:?} (interval: {:?})",
                blocks_count,
                historical_dur,
                interval_dur
            );
            let average_blocks = (target_dur.as_millis() / interval_dur.as_millis()) as u32;
            ckb_logger::trace!(
                "average: blocks count: {} in {:?}",
                average_blocks,
                target_dur,
            );
            average_blocks
        };
        if average_blocks == 0 {
            return None;
        }
        let current_txs = self.statistics.read().filter_transactions(
            |_| true,
            |_, tx| {
                let weight = get_transaction_weight(tx.size() as usize, tx.cycles());
                let fee_rate = FeeRate::calculate(Capacity::shannons(tx.fee()), weight);
                let tx_status = TxStatus::new(weight, fee_rate);
                Some(tx_status)
            },
        );
        ckb_logger::trace!("current transactions count = {}", current_txs.len());
        self.do_estimate_internal(
            probability,
            target_dur,
            historical_dur,
            current_dt,
            current_txs,
            MAX_BLOCK_WEIGHT,
            average_blocks,
        )
        .map(FeeRate::as_u64)
    }

    #[allow(clippy::too_many_arguments)]
    fn do_estimate_internal(
        &mut self,
        probability: f32,
        target_dur: Duration,
        historical_dur: Duration,
        current_dt: Duration,
        mut current_txs: Vec<TxStatus>,
        max_block_weight: u64,
        average_blocks: u32,
    ) -> Option<FeeRate> {
        current_txs.sort();
        current_txs.reverse();
        if let Some(max_fee_rate) = current_txs.first().map(|tx| tx.fee_rate) {
            ckb_logger::trace!("max fee rate of current transactions: {}", max_fee_rate);
            let max_bucket_index = Self::max_bucket_index_by_fee_rate(max_fee_rate);
            ckb_logger::trace!("current weight buckets size: {}", max_bucket_index + 1);
            let current_weight_buckets = {
                let mut current_weight = vec![0u64; max_bucket_index + 1];
                let mut index_curr = max_bucket_index;
                for tx in &current_txs {
                    let index = Self::max_bucket_index_by_fee_rate(tx.fee_rate);
                    if index < index_curr {
                        let weight_curr = current_weight[index_curr];
                        for i in current_weight.iter_mut().take(index_curr).skip(index) {
                            *i = weight_curr;
                        }
                    }
                    index_curr = index;
                    current_weight[index] += tx.weight as u64;
                }
                current_weight
            };
            for (index, bucket) in current_weight_buckets.iter().enumerate() {
                if *bucket != 0 {
                    ckb_logger::trace!(">>> current_weight[{}]: {}", index, bucket);
                }
            }
            let flow_speed_buckets = {
                let historical_dt = current_dt - historical_dur;
                let mut txs_flowed = self.txs.flowed(historical_dt);
                txs_flowed.sort();
                txs_flowed.reverse();
                let mut flowed = vec![0u64; max_bucket_index + 1];
                let mut index_curr = max_bucket_index;
                for tx in &txs_flowed {
                    let index = Self::max_bucket_index_by_fee_rate(tx.fee_rate);
                    if index < index_curr {
                        let flowed_curr = flowed[index_curr];
                        for i in flowed.iter_mut().take(index_curr).skip(index) {
                            *i = flowed_curr;
                        }
                    }
                    index_curr = index;
                    flowed[index] += tx.weight as u64;
                }
                flowed
                    .into_iter()
                    .map(|value| value / historical_dur.as_secs())
                    .collect::<Vec<_>>()
            };
            for (index, bucket) in flow_speed_buckets.iter().enumerate() {
                if *bucket != 0 {
                    ckb_logger::trace!(">>> flow_speed[{}]: {}", index, bucket);
                }
            }
            let expected_blocks = {
                let mut blocks = 0u32;
                let poisson = Poisson::new(f64::from(average_blocks)).unwrap();
                loop {
                    let expected_probability = 1.0 - poisson.cdf(u64::from(blocks));
                    if expected_probability < f64::from(probability) {
                        break;
                    }
                    blocks += 1;
                }
                u64::from(blocks)
            };
            ckb_logger::trace!("expected block count: {}", expected_blocks);
            for bucket_index in 1..(max_bucket_index + 1) {
                let current_weight = current_weight_buckets[bucket_index];
                let added_weight = flow_speed_buckets[bucket_index] * target_dur.as_secs();
                let removed_weight = max_block_weight * expected_blocks;
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
                    return Some(Self::lowest_fee_rate_by_bucket_index(bucket_index));
                }
            }
            None
        } else {
            Some(self.lowest_fee_rate)
        }
    }
}

impl FeeEstimator {
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
    use super::FeeEstimator;
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
                FeeEstimator::lowest_fee_rate_by_bucket_index(*bucket_index).as_u64();
            assert_eq!(expected_fee_rate, *fee_rate);
            let actual_bucket_index =
                FeeEstimator::max_bucket_index_by_fee_rate(FeeRate::from_u64(*fee_rate));
            assert_eq!(actual_bucket_index, *bucket_index);
        }
    }

    #[test]
    fn test_bucket_index_and_fee_rate_continuous() {
        for fee_rate in 0..3_000_000 {
            let bucket_index =
                FeeEstimator::max_bucket_index_by_fee_rate(FeeRate::from_u64(fee_rate));
            let fee_rate_le = FeeEstimator::lowest_fee_rate_by_bucket_index(bucket_index).as_u64();
            let fee_rate_gt =
                FeeEstimator::lowest_fee_rate_by_bucket_index(bucket_index + 1).as_u64();
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

pub struct FeeEstimatorController {
    sender: mpsc::Sender<((f32, u32), oneshot::Sender<EstimatorResult>)>,
    stop_handler: StopHandler<()>,
}

impl FeeEstimator {
    pub fn new() -> FeeEstimator {
        let stats = Statistics::new(LIFETIME_MINUTES);
        Self {
            lowest_fee_rate: LOWEST_FEE_RATE,
            max_target_dur: MAX_TARGET,
            boot_dt: unix_time(),
            txs: TxAddedQue(VecDeque::new()),
            validator: Validator::new(NAME),
            statistics: stats,
        }
    }

    pub fn start(
        mut self,
        async_handle: &Handle,
        notify_controller: NotifyController,
    ) -> FeeEstimatorController {
        let (sender, mut receiver) =
            mpsc::channel::<((f32, u32), oneshot::Sender<EstimatorResult>)>(100);
        let (stop_sender, mut stop) = watch::channel(WATCH_INIT);
        let stop_handler = StopHandler::new(
            SignalSender::Watch(stop_sender),
            None,
            "FeeEstimator".to_string(),
        );

        async_handle.spawn(async move {
            let mut new_block_receiver = notify_controller
                .subscribe_new_block(SUBSCRIBER_NAME.to_string())
                .await;
            let mut new_transaction_receiver = notify_controller
                .subscribe_new_transaction(SUBSCRIBER_NAME.to_string())
                .await;
            let mut reject_transaction_receiver = notify_controller
                .subscribe_reject_transaction(SUBSCRIBER_NAME.to_string())
                .await;

            loop {
                tokio::select! {
                    Some(((probability, target_minutes), resp)) = receiver.recv() => {
                        if let Err(err) = self.check_estimate_params(probability, target_minutes) {
                            let _ = resp.send(Err(err));
                        } else {
                            let fee_rate_opt = self.estimate(probability, target_minutes);
                            let _ = resp.send(Ok(fee_rate_opt));
                        }
                    },
                    Some(block) = new_block_receiver.recv() => {
                        self.commit_block(&block.into());
                    },
                    Some(tx_entry) = new_transaction_receiver.recv() => {
                        self.submit_transaction(&tx_entry.into());
                    },
                    Some(reject) = reject_transaction_receiver.recv() => {
                        self.reject_transaction(&reject.into());
                    },
                    _ = stop.changed() => break,
                    else => break,
                }
            }
        });

        FeeEstimatorController {
            sender,
            stop_handler,
        }
    }
}

impl FeeEstimator {
    fn can_estimate(&self, target_minutes: u32) -> bool {
        let target_dur = Duration::from_secs(u64::from(target_minutes) * 60);
        if target_dur > self.max_target_dur {
            false
        } else {
            let current_dt = unix_time();
            let historical_dur = Self::historical_dur(target_dur);
            let required_boot_dt = current_dt - historical_dur;
            required_boot_dt > self.boot_dt
        }
    }

    fn check_estimate_params(&self, probability: f32, target_minutes: u32) -> Result<()> {
        if probability < 0.000_001 {
            return Err(Error::invalid_params(
                "probability should not less than 0.000_001",
            ));
        }
        if probability > 0.999_999 {
            return Err(Error::invalid_params(
                "probability should not greater than 0.999_999",
            ));
        }
        if target_minutes < 1 {
            return Err(Error::invalid_params(
                "target elapsed should not less than 1 minute",
            ));
        }
        if !self.can_estimate(target_minutes) {
            return Err(Error::other("lack of empirical data"));
        }
        Ok(())
    }

    fn estimate(&mut self, probability: f32, target_minutes: u32) -> Option<u64> {
        self.do_estimate(probability, target_minutes)
    }
}

impl FeeEstimator {
    fn submit_transaction(&mut self, tx: &types::Transaction) {
        let current_dt = tx.seen_dt();
        let expired_dt = current_dt - Self::historical_dur(self.max_target_dur);
        let weight = get_transaction_weight(tx.size() as usize, tx.cycles());
        let fee_rate = FeeRate::calculate(Capacity::shannons(tx.fee()), weight);
        let new_tx = TxAdded::new(tx.hash(), weight, fee_rate, current_dt);
        self.txs.add_transaction(new_tx);
        self.txs.expire(expired_dt);
        {
            let mut minutes_opt: Option<u32> = None;
            let probability = 0.9;
            for target_minutes in Validator::<Duration>::target_minutes() {
                if self.can_estimate(*target_minutes) {
                    let result = self.estimate(probability, *target_minutes);
                    if let Some(fee_rate_tmp) = result {
                        if fee_rate >= FeeRate::from_u64(fee_rate_tmp) {
                            minutes_opt = Some(*target_minutes);
                            break;
                        }
                    }
                } else {
                    ckb_logger::trace!("new-tx: can not estimate");
                    break;
                }
            }
            if let Some(minutes) = minutes_opt {
                let expected_dt = current_dt + Duration::from_secs(u64::from(minutes) * 60);
                ckb_logger::trace!(
                    "new-tx: tx {:#x} has {:.2}% probability commit in {} minutes (before {})",
                    tx.hash(),
                    probability * 100.0,
                    minutes,
                    expected_dt.pretty()
                );
                self.validator.predict(tx.hash(), current_dt, expected_dt);
            } else {
                ckb_logger::trace!("new-tx: no suitable fee rate");
            }
        }
    }

    fn commit_block(&mut self, block: &types::Block) {
        let current_dt = block.seen_dt();
        self.validator.expire(current_dt);
        self.validator.confirm(block);
        self.validator.trace_score();
    }

    fn reject_transaction(&mut self, tx: &types::RejectedTransaction) {
        if tx.is_invalid() {
            self.txs.remove_transaction(&tx.hash());
        }
        self.validator.reject(tx);
    }
}
