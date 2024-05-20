use std::sync::Arc;

use ckb_types::{
    core::{
        tx_pool::{TxEntryInfo, TxPoolEntryInfo},
        BlockView, RecommendedFeeRates,
    },
    packed::Byte32,
};
use ckb_util::RwLock;

use crate::Error;

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

    /// Gets fee estimates.
    pub fn get_fee_estimates(
        &self,
        all_entry_info: TxPoolEntryInfo,
    ) -> Result<Option<RecommendedFeeRates>, Error> {
        match self {
            Self::Dummy => Ok(None),
            Self::ConfirmationFraction(algo) => algo.read().get_fee_estimates(),
            Self::WeightUnitsFlow(algo) => algo.read().get_fee_estimates(all_entry_info),
        }
    }
}
