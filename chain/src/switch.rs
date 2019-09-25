#![allow(clippy::unreadable_literal)]

use bitflags::bitflags;
use ckb_verification::Switch as VerificationSwitch;

bitflags! {
    pub struct Switch: u32 {
        const NONE                      = 0b00000000;
        const DISABLE_EPOCH             = 0b00000001;
        const DISABLE_UNCLES            = 0b00000010;
        const DISABLE_TWO_PHASE_COMMIT  = 0b00000100;
        const DISABLE_DAOHEADER         = 0b00001000;
        const DISABLE_REWARD            = 0b00010000;
        const DISABLE_TXS               = 0b00100000;
        const DISABLE_NON_CONTEXTUAL    = 0b01000000;
        const DISABLE_ALL               = Self::DISABLE_EPOCH.bits | Self::DISABLE_UNCLES.bits |
                                    Self::DISABLE_TWO_PHASE_COMMIT.bits | Self::DISABLE_DAOHEADER.bits |
                                    Self::DISABLE_REWARD.bits | Self::DISABLE_TXS.bits |
                                    Self::DISABLE_NON_CONTEXTUAL.bits;
    }
}

impl Switch {
    pub fn disable_all(self) -> bool {
        self.contains(Switch::DISABLE_ALL)
    }

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
    fn disable_txs(&self) -> bool {
        self.contains(Switch::DISABLE_TXS)
    }
}
