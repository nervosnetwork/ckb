use ckb_vm::elf::ProgramMetadata;
use std::num::NonZeroU64;

/// Exact cumulative mapping work requested by one already-parsed root ELF.
///
/// Each byte counts once per loader action, including overlapping actions,
/// because the production loader calls `init_pages` once for every action.
/// The receipt contains no ELF interpretation beyond the metadata consumed by
/// that loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialProgramLoadReceipt {
    action_count: usize,
    mapped_bytes: u64,
}

impl InitialProgramLoadReceipt {
    /// Derive the receipt from the exact metadata that will be passed to the
    /// production loader.
    pub fn from_metadata(metadata: &ProgramMetadata) -> Option<Self> {
        let mapped_bytes = metadata
            .actions
            .iter()
            .try_fold(0u64, |total, action| total.checked_add(action.size))?;
        Some(Self {
            action_count: metadata.actions.len(),
            mapped_bytes,
        })
    }

    /// Number of loader actions in the parsed program.
    pub const fn action_count(self) -> usize {
        self.action_count
    }

    /// Cumulative page-mapped bytes across every loader action.
    pub const fn mapped_bytes(self) -> u64 {
        self.mapped_bytes
    }
}

/// Fixed node-local upper bound for one tx-pool root-program load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialProgramLoadLimit(NonZeroU64);

impl InitialProgramLoadLimit {
    /// Construct a non-zero cumulative mapped-byte limit.
    pub const fn new(max_mapped_bytes: u64) -> Option<Self> {
        match NonZeroU64::new(max_mapped_bytes) {
            Some(limit) => Some(Self(limit)),
            None => None,
        }
    }

    /// Return the configured cumulative mapped-byte limit.
    pub const fn max_mapped_bytes(self) -> u64 {
        self.0.get()
    }

    /// Decide the already-derived receipt without inspecting ELF bytes again.
    pub const fn admits(self, receipt: InitialProgramLoadReceipt) -> bool {
        receipt.mapped_bytes <= self.max_mapped_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_vm::elf::LoadingAction;
    use proptest::prelude::*;

    fn metadata(sizes: &[u64]) -> ProgramMetadata {
        ProgramMetadata {
            actions: sizes
                .iter()
                .copied()
                .map(|size| LoadingAction {
                    addr: 0,
                    size,
                    flags: 0,
                    source: 0..0,
                    offset_from_addr: 0,
                })
                .collect(),
            entry: 0,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn initial_load_receipt_matches_the_exact_metadata_sum(
            sizes in prop::collection::vec(0u64..=u32::MAX.into(), 0..256),
        ) {
            let expected = sizes.iter().copied().sum::<u64>();
            let receipt = InitialProgramLoadReceipt::from_metadata(&metadata(&sizes))
                .expect("the bounded generated sum fits u64");
            prop_assert_eq!(receipt.action_count(), sizes.len());
            prop_assert_eq!(receipt.mapped_bytes(), expected);

            let exact = InitialProgramLoadLimit::new(expected.max(1))
                .expect("the generated exact limit is non-zero");
            prop_assert!(exact.admits(receipt));
            if expected > 1 {
                let smaller = InitialProgramLoadLimit::new(expected - 1)
                    .expect("the positive predecessor is non-zero");
                prop_assert!(!smaller.admits(receipt));
            }
        }
    }

    #[test]
    fn initial_load_receipt_rejects_arithmetic_overflow() {
        assert!(InitialProgramLoadReceipt::from_metadata(&metadata(&[u64::MAX, 1])).is_none());
        assert!(InitialProgramLoadLimit::new(0).is_none());
    }
}
