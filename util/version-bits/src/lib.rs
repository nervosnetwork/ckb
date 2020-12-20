//! Implements ckb version-bits threshold logic, and caches results.

use ckb_chain_spec::{
    consensus::Consensus,
    version_bits::{DeploymentId, ThresholdState, VERSION_BITS_TOP_MASK},
};
use ckb_store::ChainStore;
use ckb_util::Mutex;
use lazy_static::lazy_static;
use std::collections::HashMap;

use ckb_types::{
    core::{EpochExt, HeaderView, Version},
    packed,
};

lazy_static! {
    static ref THRESHOLD_STATE_CACHE: HashMap<DeploymentId, Mutex<HashMap<packed::Byte32, ThresholdState>>> = {
        let mut m = HashMap::new();
        let mut inner = HashMap::new();
        inner.insert(packed::Byte32::zero(), ThresholdState::Defined); // genesis epoch has state Defined
        m.insert(DeploymentId::DeploymentTestDummy, Mutex::new(inner));
        m
    };
}

/// Abstract struct that implements ckb version-bits threshold logic, and caches results.
pub struct VersionBitsChecker<'a, CS> {
    pub(crate) store: &'a CS,
    pub(crate) consensus: &'a Consensus,
}

impl<'a, CS: ChainStore<'a>> VersionBitsChecker<'a, CS> {
    /// Returns the state for block.
    pub fn get_state_for(&self, id: DeploymentId, block: &HeaderView) -> Option<ThresholdState> {
        let epoch = block.epoch();
        if epoch.number() == 0 {
            return Some(ThresholdState::Defined);
        }

        // block's state never depends on the version of the blocks in the same epoch;
        // only on that of the previous epoch.
        let epoch_ext = self
            .store
            .get_block_epoch_index(&block.hash())
            .and_then(|hash| self.store.get_epoch_ext(&hash))?;
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        let prev_epoch_hash = self
            .store
            .get_block_epoch_index(&last_block_hash_in_previous_epoch)?;

        let prev_epoch = self.store.get_epoch_ext(&prev_epoch_hash)?;
        self.get_state_for_epoch(id, prev_epoch, last_block_hash_in_previous_epoch)
    }

    /// Returns the state for based on previous epoch
    pub fn get_state_for_epoch(
        &self,
        id: DeploymentId,
        epoch_ext: EpochExt,
        mut last_block_hash: packed::Byte32,
    ) -> Option<ThresholdState> {
        if epoch_ext.number() == 0 {
            return Some(ThresholdState::Defined);
        }

        let threshold = (95, 100); // 95%
        let deployment = self.consensus.vbit_deployments[&id];

        let start_epoch = deployment.start_epoch;
        let timeout_epoch = deployment.timeout_epoch;

        if epoch_ext.number() < start_epoch {
            return Some(ThresholdState::Defined);
        }

        let mut cache = THRESHOLD_STATE_CACHE[&id].lock();
        let epoch_hash = epoch_ext.last_block_hash_in_previous_epoch();
        if let Some(state) = cache.get(&epoch_hash) {
            return Some(*state);
        }

        // build cache
        let mut to_compute = Vec::new();
        let mut states = Vec::new();
        let mut last_epoch_hash = epoch_hash;

        while cache.get(&last_epoch_hash).is_none() {
            let epoch_ext = self.store.get_epoch_ext(&last_epoch_hash)?;
            let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();

            if epoch_ext.number() < start_epoch {
                states.push((last_epoch_hash.clone(), ThresholdState::Defined))
            }

            to_compute.push((epoch_ext, last_block_hash));

            last_epoch_hash = self
                .store
                .get_block_epoch_index(&last_block_hash_in_previous_epoch)?;
            last_block_hash = last_block_hash_in_previous_epoch;
        }

        let mask = 1 << deployment.bit;
        let condition = |version: Version| -> bool {
            (version & VERSION_BITS_TOP_MASK == 0) && (version & mask) != 0
        };

        let mut state = *cache.get(&last_epoch_hash)?;
        for (epoch_ext, last_block_hash) in to_compute {
            let mut state_next = state;

            match state {
                ThresholdState::Defined => {
                    if epoch_ext.number() >= timeout_epoch {
                        state_next = ThresholdState::Started;
                    }
                }
                ThresholdState::Started => {
                    // we nee count
                    let mut count = 0;
                    let mut block_header = self.store.get_block_header(&last_block_hash)?;
                    while block_header.epoch().index() > 0 {
                        block_header = self.store.get_block_header(&block_header.parent_hash())?;
                        if condition(block_header.version()) {
                            count += 1;
                        }
                    }
                    let threshold_count = epoch_ext.length() * threshold.0 / threshold.1;
                    if count >= threshold_count {
                        state_next = ThresholdState::LockedIn;
                    } else if deployment.lock_in_on_timeout
                        && epoch_ext.number() + 1 >= timeout_epoch
                    {
                        state_next = ThresholdState::MustSignal;
                    } else if epoch_ext.number() >= timeout_epoch {
                        state_next = ThresholdState::Failed;
                    }
                }
                ThresholdState::MustSignal => {
                    state_next = ThresholdState::LockedIn;
                }
                ThresholdState::LockedIn => {
                    state_next = ThresholdState::Active;
                }
                ThresholdState::Active => {
                    state_next = ThresholdState::Active;
                }
                ThresholdState::Failed => {
                    state_next = ThresholdState::Failed;
                }
            }

            states.push((epoch_ext.last_block_hash_in_previous_epoch(), state_next));
            state = state_next;
        }

        cache.extend(states.into_iter());

        Some(state)
    }
}
