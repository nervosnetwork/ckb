//! The trait abstract for particular verification
use bitflags::bitflags;
use ckb_error::Error;

/// Trait for verification
pub trait Verifier {
    /// The verification associated target
    type Target;
    /// The Interface for verification
    fn verify(&self, target: &Self::Target) -> Result<(), Error>;
}

bitflags! {
    /// The bit flags for particular process block verify
    pub struct Switch: u32 {
        /// None of verifier will be disable
        const NONE                      = 0b00000000;

        /// Disable epoch verifier
        const DISABLE_EPOCH             = 0b00000001;

        /// Disable uncle verifier
        const DISABLE_UNCLES            = 0b00000010;

        /// Disable two phase commit verifier
        const DISABLE_TWO_PHASE_COMMIT  = 0b00000100;

        /// Disable dao header verifier
        const DISABLE_DAOHEADER         = 0b00001000;

        /// Disable reward verifier
        const DISABLE_REWARD            = 0b00010000;

        /// Disable non-contextual verifier
        const DISABLE_NON_CONTEXTUAL    = 0b00100000;

        /// Disable script verification
        const DISABLE_SCRIPT            = 0b01000000;

        /// Disable all verifier
        const DISABLE_ALL               = Self::DISABLE_EPOCH.bits | Self::DISABLE_UNCLES.bits |
                                    Self::DISABLE_TWO_PHASE_COMMIT.bits | Self::DISABLE_DAOHEADER.bits |
                                    Self::DISABLE_REWARD.bits |
                                    Self::DISABLE_NON_CONTEXTUAL.bits | Self::DISABLE_SCRIPT.bits;
    }
}

impl Switch {
    /// Whether all verifiers are disabled
    pub fn disable_all(self) -> bool {
        self.contains(Switch::DISABLE_ALL)
    }

    /// Whether non-contextual verifier is disabled
    pub fn disable_non_contextual(self) -> bool {
        self.contains(Switch::DISABLE_NON_CONTEXTUAL)
    }

    /// Whether epoch verifier is disabled
    pub fn disable_epoch(&self) -> bool {
        self.contains(Switch::DISABLE_EPOCH)
    }

    /// Whether uncles verifier is disabled
    pub fn disable_uncles(&self) -> bool {
        self.contains(Switch::DISABLE_UNCLES)
    }

    /// Whether two-phase-commit verifier is disabled
    pub fn disable_two_phase_commit(&self) -> bool {
        self.contains(Switch::DISABLE_TWO_PHASE_COMMIT)
    }

    /// Whether DAO-header verifier is disabled
    pub fn disable_daoheader(&self) -> bool {
        self.contains(Switch::DISABLE_DAOHEADER)
    }

    /// Whether reward verifier is disabled
    pub fn disable_reward(&self) -> bool {
        self.contains(Switch::DISABLE_REWARD)
    }

    /// Whether script verifier is disabled
    pub fn disable_script(&self) -> bool {
        self.contains(Switch::DISABLE_SCRIPT)
    }
}
