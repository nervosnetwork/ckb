//! Hardfork-related configuration

#[macro_use]
pub(crate) mod helper;
mod ckb2021;
mod ckb2023;

pub use ckb2021::{CKB2021, CKB2021Builder};
pub use ckb2023::{CKB2023, CKB2023Builder};

/// Hardfork-related configuration
#[derive(Debug, Clone)]
pub struct HardForks {
    /// ckb 2021 configuration
    pub ckb2021: CKB2021,
    /// ckb 2023 configuration
    pub ckb2023: CKB2023,
}

impl HardForks {
    /// construct mirana configuration
    pub fn new_mirana() -> HardForks {
        HardForks {
            ckb2021: CKB2021::new_mirana(),
            ckb2023: CKB2023::new_mirana(),
        }
    }

    /// construct dev configuration
    pub fn new_dev() -> HardForks {
        HardForks {
            ckb2021: CKB2021::new_dev_default(),
            ckb2023: CKB2023::new_dev_default(),
        }
    }
}
