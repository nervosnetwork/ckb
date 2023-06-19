//! Hard forks parameters.

use ckb_constant::hardfork::{mainnet, testnet};
use ckb_types::core::{
    hardfork::{CKB2021Builder, CKB2023Builder, HardForks, CKB2021, CKB2023},
    EpochNumber,
};
use serde::{Deserialize, Serialize};

/// Hard forks parameters for spec.
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardForkConfig {
    ckb2023: Option<EpochNumber>,
}

impl HardForkConfig {
    /// If all parameters which have been set are correct for mainnet, then
    /// sets all `None` to default values, otherwise, return an `Err`.
    pub fn complete_mainnet(&self) -> Result<HardForks, String> {
        let mut ckb2021 = CKB2021::new_builder();
        ckb2021 = self.update_2021(
            ckb2021,
            mainnet::CKB2021_START_EPOCH,
            mainnet::RFC0028_START_EPOCH,
        )?;

        Ok(HardForks {
            ckb2021: ckb2021.build()?,
            ckb2023: CKB2023::new_mirana().as_builder().build()?,
        })
    }

    /// If all parameters which have been set are correct for testnet, then
    /// sets all `None` to default values, otherwise, return an `Err`.
    pub fn complete_testnet(&self) -> Result<HardForks, String> {
        let mut ckb2021 = CKB2021::new_builder();
        ckb2021 = self.update_2021(
            ckb2021,
            testnet::CKB2021_START_EPOCH,
            testnet::RFC0028_START_EPOCH,
        )?;
        let mut ckb2023 = CKB2023::new_builder();
        ckb2023 = self.update_2023(ckb2023, testnet::CKB2023_START_EPOCH)?;

        Ok(HardForks {
            ckb2021: ckb2021.build()?,
            ckb2023: ckb2023.build()?,
        })
    }

    fn update_2021(
        &self,
        builder: CKB2021Builder,
        ckb2021: EpochNumber,
        rfc_0028_start: EpochNumber,
    ) -> Result<CKB2021Builder, String> {
        let builder = builder
            .rfc_0028(rfc_0028_start)
            .rfc_0029(ckb2021)
            .rfc_0030(ckb2021)
            .rfc_0031(ckb2021)
            .rfc_0032(ckb2021)
            .rfc_0036(ckb2021)
            .rfc_0038(ckb2021);
        Ok(builder)
    }

    fn update_2023(
        &self,
        builder: CKB2023Builder,
        ckb2023: EpochNumber,
    ) -> Result<CKB2023Builder, String> {
        let builder = builder.rfc_0048(ckb2023).rfc_0049(ckb2023);
        Ok(builder)
    }

    /// Converts to a hard fork switch.
    ///
    /// Enable features which are set to `None` at the dev default config.
    pub fn complete_with_dev_default(&self) -> Result<HardForks, String> {
        let mut ckb2021 = CKB2021::new_builder();
        ckb2021 = self.update_2021(
            ckb2021,
            testnet::CKB2021_START_EPOCH,
            testnet::RFC0028_START_EPOCH,
        )?;

        let ckb2023 = if let Some(epoch) = self.ckb2023 {
            CKB2023::new_with_specified(epoch)
        } else {
            CKB2023::new_dev_default()
        };

        Ok(HardForks {
            ckb2021: ckb2021.build()?,
            ckb2023,
        })
    }
}
