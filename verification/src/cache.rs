//! TX verification cache

use ckb_chain_spec::consensus::Consensus;
use ckb_script::TxVerifyEnv;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView},
    prelude::Unpack,
};

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
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct TxVerificationCacheKey {
    witness_hash: [u8; 32],
    script_rules: ScriptVerificationRules,
}

impl TxVerificationCacheKey {
    /// Bind transaction identity to the script rules under which it is
    /// verified. Neither component can be omitted by a cache caller.
    pub fn from_transaction(tx: &TransactionView, script_rules: ScriptVerificationRules) -> Self {
        Self {
            witness_hash: tx.witness_hash().unpack(),
            script_rules,
        }
    }

    /// Borrow the fixed-size witness hash. Keeping this value inline makes
    /// cache-key copies plain bounded data copies rather than shared packed
    /// buffer reference-count operations.
    pub const fn witness_hash(&self) -> &[u8; 32] {
        &self.witness_hash
    }

    /// Return the script rules bound into this cache identity.
    pub const fn script_rules(&self) -> ScriptVerificationRules {
        self.script_rules
    }
}

const CACHE_SIZE: usize = 1000 * 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VmSuccessSeal {
    cycles: Cycle,
}

/// An unforgeable proof that the exact cache key completed script execution.
///
/// The private fields and crate-private constructor keep raw cycle counts from
/// becoming script authority outside the canonical verifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptVerificationProof {
    key: TxVerificationCacheKey,
    seal: VmSuccessSeal,
}

impl ScriptVerificationProof {
    pub(crate) const fn from_vm_success(key: TxVerificationCacheKey, cycles: Cycle) -> Self {
        Self {
            key,
            seal: VmSuccessSeal { cycles },
        }
    }

    /// Return the semantic identity proved by VM execution.
    pub const fn key(&self) -> TxVerificationCacheKey {
        self.key
    }

    /// Return the successfully consumed cycles.
    pub const fn cycles(&self) -> Cycle {
        self.seal.cycles
    }
}

/// Whether canonical verification executed the VM or reused a sealed proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptVerificationOutcome {
    /// A cache proof matched the verifier-derived key and current limit.
    Reused(ScriptVerificationProof),
    /// This invocation executed the VM and produced a publishable proof.
    Executed(ScriptVerificationProof),
}

/// Fresh contextual verification result returned to block consumers.
///
/// This is an ordinary result projection, not a reusable cache value. The
/// shared cache stores only [`ScriptVerificationProof`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completed {
    /// Successfully verified script cycles for this invocation.
    pub cycles: Cycle,
    /// Fee freshly calculated from this invocation's resolved block view.
    pub fee: Capacity,
}

impl ScriptVerificationOutcome {
    /// Return the verified cycle count independent of execution mode.
    pub const fn cycles(&self) -> Cycle {
        match self {
            Self::Reused(proof) | Self::Executed(proof) => proof.cycles(),
        }
    }

    /// Return a cache update only for fresh VM success.
    pub const fn executed_proof(self) -> Option<ScriptVerificationProof> {
        match self {
            Self::Executed(proof) => Some(proof),
            Self::Reused(_) => None,
        }
    }

    /// Report whether this observation reused resident proof.
    pub const fn was_reused(&self) -> bool {
        matches!(self, Self::Reused(_))
    }
}

/// Opaque script-only verification cache.
///
/// Fee, capacity, time and DAO observations are deliberately unrepresentable
/// here. Insertion consumes one proof so key and value cannot be mismatched.
pub struct TxVerificationCache {
    inner: lru::LruCache<TxVerificationCacheKey, VmSuccessSeal>,
}

impl TxVerificationCache {
    /// Look up an exact script proof without changing LRU order under a read lock.
    pub fn lookup(&self, key: &TxVerificationCacheKey) -> Option<ScriptVerificationProof> {
        self.inner
            .peek(key)
            .copied()
            .map(|seal| ScriptVerificationProof { key: *key, seal })
    }

    /// Publish one verifier-produced proof.
    pub fn insert(&mut self, proof: ScriptVerificationProof) {
        self.inner.put(proof.key, proof.seal);
    }
}

/// Initialize cache.
pub fn init_cache() -> TxVerificationCache {
    TxVerificationCache {
        inner: lru::LruCache::new(CACHE_SIZE),
    }
}
