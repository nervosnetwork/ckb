use std::sync::Arc;

use ckb_types::{
    core::{
        tx_pool::{TxEntryInfo, TxPoolEntryInfo},
        BlockNumber, BlockView, EstimateMode, FeeRate,
    },
    packed::Byte32,
};
use ckb_util::RwLock;

use crate::{constants, Error};

mod confirmation_fraction;
mod weight_units_flow;

/// The fee estimator with a chosen algorithm.
#[derive(Clone)]
pub enum FeeEstimator {
    /// Dummy fee estimate algorithm; just do nothing.
    Dummy,
    /// Confirmation fraction fee estimator algorithm.
    ConfirmationFraction(Arc<RwLock<confirmation_fraction::Algorithm>>),
    /// Weight-Units flow fee estimator algorithm.
    WeightUnitsFlow(Arc<RwLock<weight_units_flow::Algorithm>>),
}

impl FeeEstimator {
    /// Creates a new dummy fee estimator.
    pub fn new_dummy() -> Self {
        FeeEstimator::Dummy
    }

    /// Creates a new confirmation fraction fee estimator.
    pub fn new_confirmation_fraction() -> Self {
        let algo = confirmation_fraction::Algorithm::new();
        FeeEstimator::ConfirmationFraction(Arc::new(RwLock::new(algo)))
    }

    /// Target blocks for the provided estimate mode.
    pub const fn target_blocks_for_estimate_mode(estimate_mode: EstimateMode) -> BlockNumber {
        match estimate_mode {
            EstimateMode::NoPriority => constants::DEFAULT_TARGET,
            EstimateMode::LowPriority => constants::LOW_TARGET,
            EstimateMode::MediumPriority => constants::MEDIUM_TARGET,
            EstimateMode::HighPriority => constants::HIGH_TARGET,
        }
    }

    /// Creates a new weight-units flow fee estimator.
    pub fn new_weight_units_flow() -> Self {
        let algo = weight_units_flow::Algorithm::new();
        FeeEstimator::WeightUnitsFlow(Arc::new(RwLock::new(algo)))
    }

    /// Updates the IBD state.
    pub fn update_ibd_state(&self, in_ibd: bool) {
        match self {
            Self::Dummy => {}
            Self::ConfirmationFraction(algo) => algo.write().update_ibd_state(in_ibd),
            Self::WeightUnitsFlow(algo) => algo.write().update_ibd_state(in_ibd),
        }
    }

    /// Commits a block.
    pub fn commit_block(&self, block: &BlockView) {
        match self {
            Self::Dummy => {}
            Self::ConfirmationFraction(algo) => algo.write().commit_block(block),
            Self::WeightUnitsFlow(algo) => algo.write().commit_block(block),
        }
    }

    /// Accepts a tx.
    pub fn accept_tx(&self, tx_hash: Byte32, info: TxEntryInfo) {
        match self {
            Self::Dummy => {}
            Self::ConfirmationFraction(algo) => algo.write().accept_tx(tx_hash, info),
            Self::WeightUnitsFlow(algo) => algo.write().accept_tx(info),
        }
    }

    /// Rejects a tx.
    pub fn reject_tx(&self, tx_hash: &Byte32) {
        match self {
            Self::Dummy | Self::WeightUnitsFlow(_) => {}
            Self::ConfirmationFraction(algo) => algo.write().reject_tx(tx_hash),
        }
    }

    /// Estimates fee rate.
    pub fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        all_entry_info: TxPoolEntryInfo,
    ) -> Result<FeeRate, Error> {
        let target_blocks = Self::target_blocks_for_estimate_mode(estimate_mode);
        match self {
            Self::Dummy => Err(Error::Dummy),
            Self::ConfirmationFraction(algo) => algo.read().estimate_fee_rate(target_blocks),
            Self::WeightUnitsFlow(algo) => {
                algo.read().estimate_fee_rate(target_blocks, all_entry_info)
            }
        }
    }
}
