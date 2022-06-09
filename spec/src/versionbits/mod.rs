use crate::consensus::Consensus;
use ckb_types::core::{EpochNumber, EpochNumberWithFraction, HeaderView, Ratio, Version};
use ckb_util::Mutex;
use once_cell::sync::Lazy;
use std::collections::HashMap;

pub const VERSIONBITS_TOP_BITS: Version = 0x20000000;
pub const VERSIONBITS_TOP_MASK: Version = 0xE0000000;
pub const VERSIONBITS_NUM_BITS: u32 = 29;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThresholdState {
    DEFINED,
    STARTED,
    LOCKED_IN,
    ACTIVE,
    FAILED,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ActiveMode {
    NORMAL,
    ALWAYS,
    NEVER,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum DeploymentPos {
    TESTDUMMY,
}

pub trait ConditionChecker {
    fn condition(&self, header: &HeaderView) -> bool;
    fn start(&self) -> EpochNumber;
    fn end(&self) -> EpochNumber;
    fn min_activation_epoch(&self) -> EpochNumber;
    fn period(&self) -> EpochNumber;
    fn threshold(&self) -> Ratio;
    fn active_mode(&self) -> ActiveMode;
    // fn get_stats(&self) -> SoftForkStats;
    fn get_state(&self, header: &HeaderView) -> ThresholdState;
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deployment {
    bit: u8,
    start: EpochNumber,
    timeout: EpochNumber,
    min_activation_epoch: EpochNumber,
    active_mode: ActiveMode,
}


type Cache = Mutex<HashMap<EpochNumber, ThresholdState>>;

pub struct VersionBitsCache {
    caches: HashMap<DeploymentPos, Cache>,
}


pub struct VersionBitsConditionChecker<'a> {
    id: DeploymentPos,
    consensus: &'a Consensus,
}

impl<'a> VersionBitsConditionChecker<'a> {
    fn deployment(&self) -> &Deployment {
        &self.consensus.deployments[&self.id]
    }
}

impl<'a> ConditionChecker for VersionBitsConditionChecker<'a> {
    fn condition(&self, header: &HeaderView) -> bool {
        let version = header.version();
        (((version & VERSIONBITS_TOP_MASK) == VERSIONBITS_TOP_BITS) && (version & self.mask()) != 0)
    }

    fn min_activation_epoch(&self) -> EpochNumber {
        self.deployment().min_activation_epoch
    }

    fn mask(&self) -> u32 {
        1u32 << self.deployment().bit as u32
    }

    fn active_mode(&self) -> ActiveMode {
        self.deployment().active_mode
    }

    fn start(&self) -> EpochNumber {
        self.deployment().start
    }

    fn end(&self) -> EpochNumber {
        self.deployment().timeout
    }

    fn threshold(&self) -> Ratio {
        self.consensus.soft_fork_activation_threshold
    }

    fn period(&self) -> EpochNumber {
        self.consensus.miner_confirmation_window
    }

    fn get_state(&self, header: &HeaderView) -> ThresholdState {
        let active_mode = self.active_mode();
        let start = self.start();

        if (active_mode == ActiveMode::ALWAYS) {
            return ThresholdState::ACTIVE;
        }

        if (active_mode == ActiveMode::NEVER) {
            return ThresholdState::FAILED;
        }

        if header.epoch().number() < start {
            return ThresholdState::DEFINED;
        }

        let mut cache = CACHE.lock();

        ThresholdState::FAILED
    }
}
