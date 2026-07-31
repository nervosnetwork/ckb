//! TX verification cache

use ckb_chain_spec::consensus::Consensus;
use ckb_script::{TransactionState, TxVerifyEnv};
use ckb_types::{
    core::{Capacity, Cycle, EntryCompleted, TransactionView},
    packed::Byte32,
};
use std::sync::Arc;

/// Script-rule generation under which a cached result was produced.
///
/// The witness hash identifies transaction content, but script selection also
/// changes at hard-fork boundaries. This generation is therefore part of the
/// cache key rather than metadata that each caller must remember to inspect.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum ScriptVerificationRules {
    /// CKB VM version 0 and its syscall surface.
    V0,
    /// CKB VM version 1 and syscall surface 2 are enabled.
    V1,
    /// CKB VM version 2 and syscall surface 3 are enabled.
    V2,
}

impl ScriptVerificationRules {
    /// Derive the complete script-selection generation from the same
    /// transaction environment passed to the script verifier.
    pub fn from_env(consensus: &Consensus, tx_env: &TxVerifyEnv) -> Self {
        let epoch = tx_env.epoch_number_without_proposal_window();
        let hardforks = consensus.hardfork_switch();
        if hardforks
            .ckb2023
            .is_vm_version_2_and_syscalls_3_enabled(epoch)
        {
            Self::V2
        } else if hardforks
            .ckb2021
            .is_vm_version_1_and_syscalls_2_enabled(epoch)
        {
            Self::V1
        } else {
            Self::V0
        }
    }
}

/// Semantic key for [`TxVerificationCache`].
///
/// Script verification covers witnesses, while [`TransactionView::hash`]
/// deliberately does not. Keeping the witness hash and script rules behind
/// one type makes both a raw-hash lookup and a context-free lookup impossible.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct TxVerificationCacheKey {
    witness_hash: Byte32,
    script_rules: ScriptVerificationRules,
}

impl TxVerificationCacheKey {
    /// Bind transaction identity to the script rules under which it is
    /// verified. Neither component can be omitted by a cache caller.
    pub fn from_transaction(tx: &TransactionView, script_rules: ScriptVerificationRules) -> Self {
        Self {
            witness_hash: tx.witness_hash(),
            script_rules,
        }
    }

    /// Borrow the underlying witness hash for diagnostics.
    pub fn witness_hash(&self) -> &Byte32 {
        &self.witness_hash
    }

    /// Return the script rules bound into this cache identity.
    pub const fn script_rules(&self) -> ScriptVerificationRules {
        self.script_rules
    }
}

/// TX verification lru cache.
pub type TxVerificationCache = lru::LruCache<TxVerificationCacheKey, CacheEntry>;

const CACHE_SIZE: usize = 1000 * 30;

/// Initialize cache
pub fn init_cache() -> TxVerificationCache {
    lru::LruCache::new(CACHE_SIZE)
}

/// A completed script result. Its proof context is part of the cache key.
pub type CacheEntry = Completed;

/// Suspended state
#[derive(Clone, Debug)]
pub struct Suspended {
    /// Cached tx fee
    pub fee: Capacity,
    /// State
    pub state: Arc<TransactionState>,
}

/// Completed entry
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completed {
    /// Cached tx cycles
    pub cycles: Cycle,
    /// Cached tx fee
    pub fee: Capacity,
}

impl From<Completed> for EntryCompleted {
    fn from(value: Completed) -> Self {
        EntryCompleted {
            cycles: value.cycles,
            fee: value.fee,
        }
    }
}
