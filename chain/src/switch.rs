//! TODO(doc): @zhangsoledad
#![allow(clippy::unreadable_literal)]

use bitflags::bitflags;
use ckb_verification::Switch as VerificationSwitch;

bitflags! {
    /// TODO(doc): @zhangsoledad
    pub struct Switch: u32 {
        /// TODO(doc): @zhangsoledad
        const NONE                      = 0b00000000;
        /// TODO(doc): @zhangsoledad
        const DISABLE_EPOCH             = 0b00000001;
        /// TODO(doc): @zhangsoledad
        const DISABLE_UNCLES            = 0b00000010;
        /// TODO(doc): @zhangsoledad
        const DISABLE_TWO_PHASE_COMMIT  = 0b00000100;
        /// TODO(doc): @zhangsoledad
        const DISABLE_DAOHEADER         = 0b00001000;
        /// TODO(doc): @zhangsoledad
        const DISABLE_REWARD            = 0b00010000;
        /// TODO(doc): @zhangsoledad
        const DISABLE_NON_CONTEXTUAL    = 0b00100000;
        /// TODO(doc): @zhangsoledad
        const DISABLE_ALL               = Self::DISABLE_EPOCH.bits | Self::DISABLE_UNCLES.bits |
                                    Self::DISABLE_TWO_PHASE_COMMIT.bits | Self::DISABLE_DAOHEADER.bits |
                                    Self::DISABLE_REWARD.bits |
                                    Self::DISABLE_NON_CONTEXTUAL.bits;
    }
}

impl Switch {
    /// TODO(doc): @zhangsoledad
    pub fn disable_all(self) -> bool {
        self.contains(Switch::DISABLE_ALL)
    }

    /// TODO(doc): @zhangsoledad
    pub fn disable_non_contextual(self) -> bool {
        self.contains(Switch::DISABLE_NON_CONTEXTUAL)
    }
}

impl VerificationSwitch for Switch {
    fn disable_epoch(&self) -> bool {
        self.contains(Switch::DISABLE_EPOCH)
    }
    fn disable_uncles(&self) -> bool {
        self.contains(Switch::DISABLE_UNCLES)
    }
    fn disable_two_phase_commit(&self) -> bool {
        self.contains(Switch::DISABLE_TWO_PHASE_COMMIT)
    }
    fn disable_daoheader(&self) -> bool {
        self.contains(Switch::DISABLE_DAOHEADER)
    }
    fn disable_reward(&self) -> bool {
        self.contains(Switch::DISABLE_REWARD)
    }
}
