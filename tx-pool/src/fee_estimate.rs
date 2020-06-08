use ckb_fee_estimator::Estimator as FeeEstimator;
use ckb_fee_estimator::FeeRate;
use ckb_types::{core::BlockNumber, packed::Byte32};
use std::sync::Arc;
use tokio::sync::RwLock;

pub type FeeRateSample = (BlockNumber, Vec<(Byte32, FeeRate)>);

pub async fn track_tx(fee_estimator: Arc<RwLock<FeeEstimator>>, sample: FeeRateSample) {
    let (height, sample) = sample;
    let mut guard = fee_estimator.write().await;
    for (tx_hash, fee_rate) in sample {
        guard.track_tx(tx_hash, fee_rate, height);
    }
}

pub async fn estimate(fee_estimator: Arc<RwLock<FeeEstimator>>, expect_confirm: usize) -> FeeRate {
    let guard = fee_estimator.read().await;
    guard.estimate(expect_confirm)
}

pub async fn process_block(
    fee_estimator: Arc<RwLock<FeeEstimator>>,
    height: BlockNumber,
    txs: impl Iterator<Item = Byte32>,
) {
    let mut guard = fee_estimator.write().await;
    guard.process_block(height, txs)
}
