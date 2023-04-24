use crate::core::EpochNumber;
use ckb_constant::hardfork;
use paste::paste;

/// A switch to select hard fork features base on the epoch number.
///
/// For safety, all fields are private and not allowed to update.
/// This structure can only be constructed by [`CKB2023Builder`].
///
/// [`CKB2023Builder`]:  struct.CKB2023Builder.html
#[derive(Debug, Clone)]
pub struct CKB2023 {
    rfc_0146: EpochNumber,
    rfc_0148: EpochNumber,
}

/// Builder for [`CKB2023`].
///
/// [`CKB2023`]:  struct.CKB2023.html
#[derive(Debug, Clone, Default)]
pub struct CKB2023Builder {
    rfc_0146: Option<EpochNumber>,
    rfc_0148: Option<EpochNumber>,
}

impl CKB2023 {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> CKB2023Builder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> CKB2023Builder {
        Self::new_builder()
            .rfc_0146(self.rfc_0146())
            .rfc_0148(self.rfc_0148())
    }

    /// Creates a new mirana instance.
    pub fn new_mirana() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder()
            .rfc_0146(hardfork::mainnet::CKB2023_START_EPOCH)
            .rfc_0148(hardfork::mainnet::CKB2023_START_EPOCH)
            .build()
            .unwrap()
    }
}

define_methods!(
    CKB2023,
    rfc_0146,
    remove_header_version_reservation_rule,
    is_remove_header_version_reservation_rule_enabled,
    disable_rfc_0146,
    "RFC PR 0146"
);
define_methods!(
    CKB2023,
    rfc_0148,
    vm_version_2_and_syscalls_3,
    is_vm_version_2_and_syscalls_3_enabled,
    disable_rfc_0148,
    "RFC PR 0148"
);

impl CKB2023Builder {
    /// Build a new [`CKB2023`].
    ///
    /// Returns an error if failed at any check, for example, there maybe are some features depend
    /// on others.
    ///
    /// [`CKB2023`]: struct.CKB2023.html
    pub fn build(self) -> Result<CKB2023, String> {
        let rfc_0146 = try_find!(self, rfc_0146);
        let rfc_0148 = try_find!(self, rfc_0148);

        Ok(CKB2023 { rfc_0146, rfc_0148 })
    }
}
