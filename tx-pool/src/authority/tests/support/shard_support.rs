use std::hash::{Hash, Hasher};

pub(in crate::authority) const AUTHORITY_SHARD_COUNT: usize = 64;

/// Executable pre-migration view of the fixed R3 shard support.
///
/// The production layout will own a per-runtime randomized hasher.  This
/// deterministic test router exists only to prove that real typed deltas can
/// derive their complete support without a caller-provided route vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct AuthorityShardSupport(u64);

impl AuthorityShardSupport {
    pub(in crate::authority) fn insert<K: Hash>(&mut self, domain: &'static [u8], key: &K) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        domain.hash(&mut hasher);
        key.hash(&mut hasher);
        let shard = hasher.finish() % AUTHORITY_SHARD_COUNT as u64;
        self.0 |= 1u64 << shard;
    }

    pub(in crate::authority) fn is_disjoint(self, other: Self) -> bool {
        self.0 & other.0 == 0
    }

    pub(in crate::authority) fn union(&mut self, other: Self) -> bool {
        let before = self.0;
        self.0 |= other.0;
        self.0 != before
    }

    pub(in crate::authority) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(in crate::authority) fn len(self) -> usize {
        self.0.count_ones() as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct ExclusiveSupport {
    pub(in crate::authority) membership_counts: bool,
    pub(in crate::authority) source_versions: bool,
    pub(in crate::authority) scheduler_cursor: bool,
    pub(in crate::authority) dependency_control: bool,
    pub(in crate::authority) effect_log: bool,
    pub(in crate::authority) clocks: bool,
}

/// Working finite-progress rule for support discovered after acquisition.
/// Each non-complete transition adds at least one of the fixed 64 shards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum SupportExpansion {
    Complete,
    AcquireHigher(AuthorityShardSupport),
    ReleaseAndReacquire(AuthorityShardSupport),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct MonotoneSupportClosure {
    required: AuthorityShardSupport,
    held: AuthorityShardSupport,
}

impl MonotoneSupportClosure {
    pub(in crate::authority) fn discover(
        &mut self,
        observed: AuthorityShardSupport,
    ) -> SupportExpansion {
        self.required.union(observed);
        if self.held.contains(self.required) {
            return SupportExpansion::Complete;
        }
        let missing_bits = self.required.0 & !self.held.0;
        let missing = AuthorityShardSupport(missing_bits);
        let highest_held = 63u32.saturating_sub(self.held.0.leading_zeros());
        let has_lower_missing = self.held.0 != 0 && missing_bits.trailing_zeros() < highest_held;
        if has_lower_missing {
            self.held = self.required;
            SupportExpansion::ReleaseAndReacquire(self.required)
        } else {
            self.held.union(missing);
            SupportExpansion::AcquireHigher(missing)
        }
    }

    pub(in crate::authority) fn held(self) -> AuthorityShardSupport {
        self.held
    }
}

#[test]
fn monotone_support_closure_finishes_in_at_most_the_fixed_shard_count() {
    let mut closure = MonotoneSupportClosure::default();
    let mut expansions = 0usize;
    for shard in (0..AUTHORITY_SHARD_COUNT).rev() {
        let support = AuthorityShardSupport(1u64 << shard);
        if !matches!(closure.discover(support), SupportExpansion::Complete) {
            expansions += 1;
        }
    }
    assert_eq!(closure.held().len(), AUTHORITY_SHARD_COUNT);
    assert!(expansions <= AUTHORITY_SHARD_COUNT);
    assert_eq!(
        closure.discover(AuthorityShardSupport(u64::MAX)),
        SupportExpansion::Complete
    );
}
