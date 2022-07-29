use crate::consensus::Consensus;
use ckb_types::{
    core::{EpochExt, EpochNumber, HeaderView, Ratio, TransactionView, Version},
    packed::{Byte32, CellbaseWitnessReader},
    prelude::*,
};
use ckb_util::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// What bits to set in version for versionbits blocks
pub const VERSIONBITS_TOP_BITS: Version = 0x20000000;
/// What bitmask determines whether versionbits is in use
pub const VERSIONBITS_TOP_MASK: Version = 0xE0000000;
/// Total bits available for versionbits
pub const VERSIONBITS_NUM_BITS: u32 = 29;

/// RFC0000 defines a finite-state-machine to deploy a soft fork in multiple stages.
/// State transitions happen during epoch if conditions are met
/// In case of reorg, transitions can go backward. Without transition, state is
/// inherited between epochs. All blocks of a epoch share the same state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThresholdState {
    Defined,
    Started,
    LockedIn,
    Active,
    Failed,
}

/// This is useful for testing, as it means tests don't need to deal with the activation
/// process. Only tests that specifically test the behaviour during activation cannot use this.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ActiveMode {
    Normal,
    Always,
    Never,
}

// Soft fork deployment
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum DeploymentPos {
    Testdummy,
    LightClient,
}

/// VersionBitsIndexer
pub trait VersionBitsIndexer {
    fn get_block_epoch_index(&self, block_hash: &Byte32) -> Option<Byte32>;
    fn get_epoch_ext(&self, index: &Byte32) -> Option<EpochExt>;
    fn get_block_header(&self, block_hash: &Byte32) -> Option<HeaderView>;
    fn get_cellbase(&self, block_hash: &Byte32) -> Option<TransactionView>;
    fn get_ancestor_epoch(&self, index: &Byte32, period: EpochNumber) -> Option<EpochExt>;
}

///Struct for each individual consensus rule change using soft fork.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deployment {
    pub(crate) bit: u8,
    pub(crate) start: EpochNumber,
    pub(crate) timeout: EpochNumber,
    pub(crate) min_activation_epoch: EpochNumber,
    pub(crate) period: EpochNumber,
    pub(crate) active_mode: ActiveMode,
    pub(crate) threshold: Ratio,
}

type Cache = Mutex<HashMap<Byte32, ThresholdState>>;

/// RFC0000 allows multiple soft forks to be deployed in parallel. We cache
/// per-epoch state for every one of them. */
#[derive(Clone, Debug, Default)]
pub struct VersionBitsCache {
    caches: Arc<HashMap<DeploymentPos, Cache>>,
}

impl VersionBitsCache {
    pub fn new<'a>(deployments: impl Iterator<Item = &'a DeploymentPos>) -> Self {
        let caches: HashMap<_, _> = deployments
            .map(|pos| (*pos, Mutex::new(HashMap::new())))
            .collect();
        VersionBitsCache {
            caches: Arc::new(caches),
        }
    }

    pub fn cache(&self, pos: &DeploymentPos) -> &Cache {
        &self.caches[pos]
    }
}

/// implements RFC0000 threshold logic, and caches results.
pub struct VersionBits<'a> {
    id: DeploymentPos,
    consensus: &'a Consensus,
}

pub trait VersionBitsConditionChecker {
    fn start(&self) -> EpochNumber;
    fn timeout(&self) -> EpochNumber;
    fn active_mode(&self) -> ActiveMode;
    // fn condition(&self, header: &HeaderView) -> bool;
    fn condition<I: VersionBitsIndexer>(&self, header: &HeaderView, indexer: &I) -> bool;
    fn min_activation_epoch(&self) -> EpochNumber;
    fn period(&self) -> EpochNumber;
    fn threshold(&self) -> Ratio;

    fn get_state<I: VersionBitsIndexer>(
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

        let start_index = indexer.get_block_epoch_index(&header.hash())?;
        let epoch_number = header.epoch().number();
        let target = epoch_number.saturating_sub((epoch_number + 1) % period);

        let mut epoch_ext = indexer.get_ancestor_epoch(&start_index, target)?;
        let mut epoch_index = epoch_ext.last_block_hash_in_previous_epoch();
        let mut g_cache = cache.lock();
        let mut to_compute = Vec::new();
        while g_cache.get(&epoch_index).is_none() {
            if epoch_ext.is_genesis() {
                // The genesis is by definition defined.
                g_cache.insert(epoch_index.clone(), ThresholdState::Defined);
                break;
            }
            if epoch_ext.number() < start {
                // The genesis is by definition defined.
                g_cache.insert(epoch_index.clone(), ThresholdState::Defined);
                break;
            }
            to_compute.push(epoch_ext.clone());

            let next_epoch_ext = indexer
                .get_ancestor_epoch(&epoch_index, epoch_ext.number().saturating_sub(period))?;
            epoch_ext = next_epoch_ext;
            epoch_index = epoch_ext.last_block_hash_in_previous_epoch();
        }

        let mut state = *g_cache
            .get(&epoch_index)
            .expect("cache[epoch_index] is known");
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
                        indexer.get_block_header(&epoch_ext.last_block_hash_in_previous_epoch())?;

                    let mut current_epoch_ext = epoch_ext.clone();
                    for _ in 0..period {
                        let current_epoch_length = current_epoch_ext.length();
                        total += current_epoch_length;
                        for _ in 0..current_epoch_length {
                            if self.condition(&header, indexer) {
                                count += 1;
                            }
                            header = indexer.get_block_header(&header.parent_hash())?;
                        }
                        let last_block_header_in_previous_epoch = indexer.get_block_header(
                            &current_epoch_ext.last_block_hash_in_previous_epoch(),
                        )?;
                        let previous_epoch_index = indexer
                            .get_block_epoch_index(&last_block_header_in_previous_epoch.hash())?;
                        current_epoch_ext = indexer.get_epoch_ext(&previous_epoch_index)?;
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
}

impl<'a> VersionBits<'a> {
    pub fn new(id: DeploymentPos, consensus: &'a Consensus) -> Self {
        VersionBits { id, consensus }
    }

    fn deployment(&self) -> &Deployment {
        &self.consensus.deployments[&self.id]
    }

    pub fn mask(&self) -> u32 {
        1u32 << self.deployment().bit as u32
    }
}

impl<'a> VersionBitsConditionChecker for VersionBits<'a> {
    fn start(&self) -> EpochNumber {
        self.deployment().start
    }

    fn timeout(&self) -> EpochNumber {
        self.deployment().timeout
    }

    fn period(&self) -> EpochNumber {
        self.deployment().period
    }

    fn condition<I: VersionBitsIndexer>(&self, header: &HeaderView, indexer: &I) -> bool {
        if let Some(cellbase) = indexer.get_cellbase(&header.hash()) {
            if let Some(witness) = cellbase.witnesses().get(0) {
                if let Some(reader) = CellbaseWitnessReader::from_slice(&witness.raw_data()).ok() {
                    let message = reader.message().to_entity();
                    if message.len() >= 4 {
                        if let Ok(raw) = message.as_slice()[..4].try_into() {
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
