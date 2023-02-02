//! Versionbits 9 defines a finite-state-machine to deploy a softfork in multiple stages.
//!

mod convert;

use crate::consensus::Consensus;
use ckb_types::{
    core::{EpochExt, EpochNumber, HeaderView, Ratio, TransactionView, Version},
    packed::{Byte32, CellbaseWitnessReader},
    prelude::*,
};
use ckb_util::Mutex;
use std::collections::{hash_map, HashMap};
use std::sync::Arc;

/// What bits to set in version for versionbits blocks
pub const VERSIONBITS_TOP_BITS: Version = 0x00000000;
/// What bitmask determines whether versionbits is in use
pub const VERSIONBITS_TOP_MASK: Version = 0xE0000000;
/// Total bits available for versionbits
pub const VERSIONBITS_NUM_BITS: u32 = 29;

/// RFC0043 defines a finite-state-machine to deploy a soft fork in multiple stages.
/// State transitions happen during epoch if conditions are met
/// In case of reorg, transitions can go backward. Without transition, state is
/// inherited between epochs. All blocks of a epoch share the same state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThresholdState {
    /// First state that each softfork starts.
    /// The 0 epoch is by definition in this state for each deployment.
    Defined,
    /// For epochs past the `start` epoch.
    Started,
    /// For one epoch after the first epoch period with STARTED epochs of
    /// which at least `threshold` has the associated bit set in `version`.
    LockedIn,
    /// For all epochs after the LOCKED_IN epoch.
    Active,
    /// For one epoch period past the `timeout_epoch`, if LOCKED_IN was not reached.
    Failed,
}

/// This is useful for testing, as it means tests don't need to deal with the activation
/// process. Only tests that specifically test the behaviour during activation cannot use this.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ActiveMode {
    /// Indicating that the deployment is normal active.
    Normal,
    /// Indicating that the deployment is always active.
    /// This is useful for testing, as it means tests don't need to deal with the activation
    Always,
    /// Indicating that the deployment is never active.
    /// This is useful for testing.
    Never,
}

/// Soft fork deployment
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum DeploymentPos {
    /// Dummy
    Testdummy,
    /// light client protocol
    LightClient,
}

/// VersionbitsIndexer
pub trait VersionbitsIndexer {
    /// Gets epoch index by block hash
    fn block_epoch_index(&self, block_hash: &Byte32) -> Option<Byte32>;
    /// Gets epoch ext by index
    fn epoch_ext(&self, index: &Byte32) -> Option<EpochExt>;
    /// Gets block header by block hash
    fn block_header(&self, block_hash: &Byte32) -> Option<HeaderView>;
    /// Gets cellbase by block hash
    fn cellbase(&self, block_hash: &Byte32) -> Option<TransactionView>;
    /// Gets ancestor of specified epoch.
    fn ancestor_epoch(&self, index: &Byte32, target: EpochNumber) -> Option<EpochExt> {
        let mut epoch_ext = self.epoch_ext(index)?;

        if epoch_ext.number() < target {
            return None;
        }
        while epoch_ext.number() > target {
            let last_block_header_in_previous_epoch =
                self.block_header(&epoch_ext.last_block_hash_in_previous_epoch())?;
            let previous_epoch_index =
                self.block_epoch_index(&last_block_header_in_previous_epoch.hash())?;
            epoch_ext = self.epoch_ext(&previous_epoch_index)?;
        }
        Some(epoch_ext)
    }
}

///Struct for each individual consensus rule change using soft fork.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deployment {
    /// Determines which bit in the `version` field of the block is to be used to signal the softfork lock-in and activation.
    /// It is chosen from the set {0,1,2,...,28}.
    pub bit: u8,
    /// Specifies the first epoch in which the bit gains meaning.
    pub start: EpochNumber,
    /// Specifies an epoch at which the miner signaling ends.
    /// Once this epoch has been reached, if the softfork has not yet locked_in (excluding this epoch block's bit state),
    /// the deployment is considered failed on all descendants of the block.
    pub timeout: EpochNumber,
    /// Specifies the epoch at which the softfork is allowed to become active.
    pub min_activation_epoch: EpochNumber,
    /// Specifies length of epochs of the signalling period.
    pub period: EpochNumber,
    /// This is useful for testing, as it means tests don't need to deal with the activation process
    pub active_mode: ActiveMode,
    /// Specifies the minimum ratio of block per `period`,
    /// which indicate the locked_in of the softfork during the `period`.
    pub threshold: Ratio,
}

type Cache = Mutex<HashMap<Byte32, ThresholdState>>;

/// RFC0000 allows multiple soft forks to be deployed in parallel. We cache
/// per-epoch state for every one of them. */
#[derive(Clone, Debug, Default)]
pub struct VersionbitsCache {
    caches: Arc<HashMap<DeploymentPos, Cache>>,
}

impl VersionbitsCache {
    /// Construct new VersionbitsCache instance from deployments
    pub fn new<'a>(deployments: impl Iterator<Item = &'a DeploymentPos>) -> Self {
        let caches: HashMap<_, _> = deployments
            .map(|pos| (*pos, Mutex::new(HashMap::new())))
            .collect();
        VersionbitsCache {
            caches: Arc::new(caches),
        }
    }

    /// Returns a reference to the cache corresponding to the deployment.
    pub fn cache(&self, pos: &DeploymentPos) -> Option<&Cache> {
        self.caches.get(pos)
    }
}

/// Struct Implements versionbits threshold logic, and caches results.
pub struct Versionbits<'a> {
    id: DeploymentPos,
    consensus: &'a Consensus,
}

/// Trait that implements versionbits threshold logic, and caches results.
pub trait VersionbitsConditionChecker {
    /// Specifies the first epoch in which the bit gains meaning.
    fn start(&self) -> EpochNumber;
    /// Specifies an epoch at which the miner signaling ends.
    /// Once this epoch has been reached,
    /// if the softfork has not yet locked_in (excluding this epoch block's bit state),
    /// the deployment is considered failed on all descendants of the block.
    fn timeout(&self) -> EpochNumber;
    /// Active mode for testing.
    fn active_mode(&self) -> ActiveMode;
    // fn condition(&self, header: &HeaderView) -> bool;
    /// Determines whether bit in the `version` field of the block is to be used to signal
    fn condition<I: VersionbitsIndexer>(&self, header: &HeaderView, indexer: &I) -> bool;
    /// Specifies the epoch at which the softfork is allowed to become active.
    fn min_activation_epoch(&self) -> EpochNumber;
    /// The period for signal statistics are counted
    fn period(&self) -> EpochNumber;
    /// Specifies the minimum ratio of block per epoch,
    /// which indicate the locked_in of the softfork during the epoch.
    fn threshold(&self) -> Ratio;
    /// Returns the state for a header. Applies any state transition if conditions are present.
    /// Caches state from first block of period.
    fn get_state<I: VersionbitsIndexer>(
        &self,
        header: &HeaderView,
        cache: &Cache,
        indexer: &I,
    ) -> Option<ThresholdState> {
        let active_mode = self.active_mode();
        let start = self.start();
        let timeout = self.timeout();
        let period = self.period();
        let min_activation_epoch = self.min_activation_epoch();

        if active_mode == ActiveMode::Always {
            return Some(ThresholdState::Active);
        }

        if active_mode == ActiveMode::Never {
            return Some(ThresholdState::Failed);
        }

        let start_index = indexer.block_epoch_index(&header.hash())?;
        let epoch_number = header.epoch().number();
        let target = epoch_number.saturating_sub((epoch_number + 1) % period);

        let mut epoch_ext = indexer.ancestor_epoch(&start_index, target)?;
        let mut g_cache = cache.lock();
        let mut to_compute = Vec::new();
        let mut state = loop {
            let epoch_index = epoch_ext.last_block_hash_in_previous_epoch();
            match g_cache.entry(epoch_index.clone()) {
                hash_map::Entry::Occupied(entry) => {
                    break *entry.get();
                }
                hash_map::Entry::Vacant(entry) => {
                    // The genesis is by definition defined.
                    if epoch_ext.is_genesis() || epoch_ext.number() < start {
                        entry.insert(ThresholdState::Defined);
                        break ThresholdState::Defined;
                    }
                    let next_epoch_ext = indexer
                        .ancestor_epoch(&epoch_index, epoch_ext.number().saturating_sub(period))?;
                    to_compute.push(epoch_ext);
                    epoch_ext = next_epoch_ext;
                }
            }
        };

        while let Some(epoch_ext) = to_compute.pop() {
            let mut next_state = state;

            match state {
                ThresholdState::Defined => {
                    if epoch_ext.number() >= start {
                        next_state = ThresholdState::Started;
                    }
                }
                ThresholdState::Started => {
                    // We need to count
                    debug_assert!(epoch_ext.number() + 1 >= period);

                    let mut count = 0;
                    let mut total = 0;
                    let mut header =
                        indexer.block_header(&epoch_ext.last_block_hash_in_previous_epoch())?;

                    let mut current_epoch_ext = epoch_ext.clone();
                    for _ in 0..period {
                        let current_epoch_length = current_epoch_ext.length();
                        total += current_epoch_length;
                        for _ in 0..current_epoch_length {
                            if self.condition(&header, indexer) {
                                count += 1;
                            }
                            header = indexer.block_header(&header.parent_hash())?;
                        }
                        let last_block_header_in_previous_epoch = indexer
                            .block_header(&current_epoch_ext.last_block_hash_in_previous_epoch())?;
                        let previous_epoch_index = indexer
                            .block_epoch_index(&last_block_header_in_previous_epoch.hash())?;
                        current_epoch_ext = indexer.epoch_ext(&previous_epoch_index)?;
                    }

                    let threshold_number = threshold_number(total, self.threshold())?;
                    if count >= threshold_number {
                        next_state = ThresholdState::LockedIn;
                    } else if epoch_ext.number() >= timeout {
                        next_state = ThresholdState::Failed;
                    }
                }
                ThresholdState::LockedIn => {
                    if epoch_ext.number() >= min_activation_epoch {
                        next_state = ThresholdState::Active;
                    }
                }
                ThresholdState::Failed | ThresholdState::Active => {
                    // Nothing happens, these are terminal states.
                }
            }
            state = next_state;
            g_cache.insert(epoch_ext.last_block_hash_in_previous_epoch(), state);
        }

        Some(state)
    }

    /// Returns the first epoch which the state applies
    fn get_state_since_epoch<I: VersionbitsIndexer>(
        &self,
        header: &HeaderView,
        cache: &Cache,
        indexer: &I,
    ) -> Option<EpochNumber> {
        if matches!(self.active_mode(), ActiveMode::Always | ActiveMode::Never) {
            return Some(0);
        }
        let period = self.period();

        let init_state = self.get_state(header, cache, indexer)?;
        if init_state == ThresholdState::Defined {
            return Some(0);
        }

        if init_state == ThresholdState::Started {
            return Some(self.start());
        }

        let index = indexer.block_epoch_index(&header.hash())?;
        let epoch_number = header.epoch().number();
        let period_start = epoch_number.saturating_sub((epoch_number + 1) % period);

        let mut epoch_ext = indexer.ancestor_epoch(&index, period_start)?;
        let mut epoch_index = epoch_ext.last_block_hash_in_previous_epoch();
        let g_cache = cache.lock();

        while let Some(prev_epoch_ext) =
            indexer.ancestor_epoch(&epoch_index, epoch_ext.number().saturating_sub(period))
        {
            epoch_ext = prev_epoch_ext;
            epoch_index = epoch_ext.last_block_hash_in_previous_epoch();

            if let Some(state) = g_cache.get(&epoch_index) {
                if state != &init_state {
                    break;
                }
            } else {
                break;
            }
        }

        Some(epoch_ext.number().saturating_add(period))
    }
}

impl<'a> Versionbits<'a> {
    /// construct new Versionbits wrapper
    pub fn new(id: DeploymentPos, consensus: &'a Consensus) -> Self {
        Versionbits { id, consensus }
    }

    fn deployment(&self) -> &Deployment {
        &self.consensus.deployments[&self.id]
    }

    /// return bit mask corresponding deployment
    pub fn mask(&self) -> u32 {
        1u32 << self.deployment().bit as u32
    }
}

impl<'a> VersionbitsConditionChecker for Versionbits<'a> {
    fn start(&self) -> EpochNumber {
        self.deployment().start
    }

    fn timeout(&self) -> EpochNumber {
        self.deployment().timeout
    }

    fn period(&self) -> EpochNumber {
        self.deployment().period
    }

    fn condition<I: VersionbitsIndexer>(&self, header: &HeaderView, indexer: &I) -> bool {
        if let Some(cellbase) = indexer.cellbase(&header.hash()) {
            if let Some(witness) = cellbase.witnesses().get(0) {
                if let Ok(reader) = CellbaseWitnessReader::from_slice(&witness.raw_data()) {
                    let message = reader.message().to_entity();
                    if message.len() >= 4 {
                        if let Ok(raw) = message.raw_data()[..4].try_into() {
                            let version = u32::from_le_bytes(raw);
                            return ((version & VERSIONBITS_TOP_MASK) == VERSIONBITS_TOP_BITS)
                                && (version & self.mask()) != 0;
                        }
                    }
                }
            }
        }
        false
    }

    // fn condition(&self, header: &HeaderView) -> bool {
    //     let version = header.version();
    //     (((version & VERSIONBITS_TOP_MASK) == VERSIONBITS_TOP_BITS) && (version & self.mask()) != 0)
    // }

    fn min_activation_epoch(&self) -> EpochNumber {
        self.deployment().min_activation_epoch
    }

    fn active_mode(&self) -> ActiveMode {
        self.deployment().active_mode
    }

    fn threshold(&self) -> Ratio {
        self.deployment().threshold
    }
}

fn threshold_number(length: u64, threshold: Ratio) -> Option<u64> {
    length
        .checked_mul(threshold.numer())
        .and_then(|ret| ret.checked_div(threshold.denom()))
}
