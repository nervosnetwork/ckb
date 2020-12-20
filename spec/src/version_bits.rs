use ckb_types::core::{EpochNumber, Version};

/// bit mask determines whether version bits is in use
pub const VERSION_BITS_TOP_MASK: Version = 0xE0000000;

/// ThresholdState defines a finite-state-machine to deploy a softfork in multiple stages.
#[derive(Debug, Clone, Copy)]
pub enum ThresholdState {
    /// First state that each softfork starts out as. The genesis block is by definition in this state for each deployment.
    Defined,
    /// For blocks past the start epoch.
    Started,
    /// If `lock_in_on_timeout` is true, the period immediately before `timeout_epoch` unless LockedIn is reached first
    MustSignal,
    /// For one re-target period after the first re-target period with `Started` blocks of which at least threshold have the associated bit set in version.
    LockedIn,
    /// For all blocks after the `LockedIn` re-target period (final state)
    Active,
    /// For all blocks once the first re-target period after the timeout_epoch is hit, if `LockedIn` wasn't already reached (final state)
    Failed,
}

// NOTE: Also add new deployments to util/version-bits
/// Deployment id for `Deployment`
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DeploymentId {
    /// Dummy id for test
    DeploymentTestDummy,
}

/// Struct for each individual consensus rule change using
/// https://github.com/doitian/rfcs/blob/user-activated-soft-forks/rfcs/0000-user-activated-soft-forks/0000-user-activated-soft-forks.md
#[derive(Debug, Clone, Copy)]
pub struct Deployment {
    /// Bit position to select the particular bit in version.
    pub bit: u32,
    /// Start epoch for version bits miner confirmation.
    pub start_epoch: EpochNumber,
    /// Timeout/expiry epoch for the deployment attempt
    pub timeout_epoch: EpochNumber,
    /// If true, final period before timeout will transition to MustSignal.
    pub lock_in_on_timeout: bool,
}
