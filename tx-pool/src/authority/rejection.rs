//! Exact, bounded reasons produced by final membership planning.
//!
//! These values are transaction outcomes, not authority faults.  Keeping the
//! closed domain outside both the membership projection and effect log lets a
//! single candidate-disposition Plan carry the same reason through owner
//! terminalization and committed publication without translating it to a
//! lossy class in between.

use super::state::RawTxHash;
use crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
use crate::error::Reject;
use ckb_jsonrpc_types::PoolTransactionReject;
use ckb_types::{
    core::{Capacity, FeeRate, error::OutPointError},
    packed::OutPoint,
};

const MAX_DYNAMIC_REJECT_TEXT_BYTES: usize = MAX_TX_POOL_REJECT_DESCRIPTION_BYTES - 128;

/// Bounded, stable rejection evidence that may cross the committed-effect
/// boundary.
///
/// `Reject::Verification` can retain a dynamically typed error graph and the
/// string-bearing variants may contain attacker-shaped diagnostics.  The
/// authority journal must not retain either without a bound.  This wrapper
/// preserves the existing public reject variant and policy decisions while
/// replacing only an over-limit diagnostic with a bounded value of the same
/// variant. Equality follows the public RPC representation plus the policy
/// flags consumed by the endpoint adapter, not pointer identity inside an
/// error graph.
#[derive(Clone)]
pub(super) struct CommittedPublicReject {
    reject: Reject,
    malformed: bool,
    recordable: bool,
    relay_allowed: bool,
    description_bytes: usize,
}

impl std::fmt::Debug for CommittedPublicReject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedPublicReject")
            .field("reject", &self.reject)
            .field("malformed", &self.malformed)
            .field("recordable", &self.recordable)
            .field("relay_allowed", &self.relay_allowed)
            .finish()
    }
}

impl PartialEq for CommittedPublicReject {
    fn eq(&self, other: &Self) -> bool {
        PoolTransactionReject::from(self.reject.clone())
            == PoolTransactionReject::from(other.reject.clone())
            && self.malformed == other.malformed
            && self.recordable == other.recordable
            && self.relay_allowed == other.relay_allowed
    }
}

impl Eq for CommittedPublicReject {}

impl CommittedPublicReject {
    pub(super) fn new(reject: Reject) -> Self {
        let malformed = reject.is_malformed_tx();
        let recordable = reject.should_recorded();
        let relay_allowed = reject.is_allowed_relay();
        let reject = bound_reject_diagnostic(reject);
        let description_bytes =
            public_description(&PoolTransactionReject::from(reject.clone())).len();
        Self {
            reject,
            malformed,
            recordable,
            relay_allowed,
            description_bytes,
        }
    }

    pub(super) fn reject(&self) -> &Reject {
        &self.reject
    }

    pub(super) const fn is_malformed(&self) -> bool {
        self.malformed
    }

    pub(super) const fn should_record(&self) -> bool {
        self.recordable
    }

    pub(super) const fn relay_allowed(&self) -> bool {
        self.relay_allowed
    }

    pub(super) const fn description_bytes(&self) -> usize {
        self.description_bytes
    }
}

impl From<Reject> for CommittedPublicReject {
    fn from(reject: Reject) -> Self {
        Self::new(reject)
    }
}

#[cfg(test)]
impl From<super::state::RejectionKind> for CommittedPublicReject {
    fn from(reason: super::state::RejectionKind) -> Self {
        let reject = match reason {
            super::state::RejectionKind::Verification => {
                Reject::Invalidated("foundation verification rejection".to_owned())
            }
            super::state::RejectionKind::Policy => {
                Reject::Invalidated("foundation policy rejection".to_owned())
            }
        };
        Self::new(reject)
    }
}

fn bounded_text(text: String, limit: usize) -> String {
    let boundary = text.floor_char_boundary(text.len().min(limit));
    if boundary == text.len() && text.capacity() <= limit {
        text
    } else {
        text[..boundary].to_owned()
    }
}

fn bound_reject_diagnostic(reject: Reject) -> Reject {
    match reject {
        Reject::Full(message) => Reject::Full(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES)),
        Reject::Malformed(kind, message) => {
            let half = MAX_DYNAMIC_REJECT_TEXT_BYTES / 2;
            Reject::Malformed(bounded_text(kind, half), bounded_text(message, half))
        }
        // Never retain a foreign dynamic error graph in the committed
        // journal, even when its Display text happens to be short. The typed
        // error kind and a detached bounded diagnostic are the complete
        // public contract.
        Reject::Verification(error) => Reject::Verification(error.kind().other(bounded_text(
            error.to_string(),
            MAX_DYNAMIC_REJECT_TEXT_BYTES,
        ))),
        Reject::RBFRejected(message) => {
            Reject::RBFRejected(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        Reject::Invalidated(message) => {
            Reject::Invalidated(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        Reject::Internal(message) => {
            Reject::Internal(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        fixed @ (Reject::LowFeeRate(..)
        | Reject::ExceededMaximumAncestorsCount
        | Reject::ExceededTransactionSizeLimit(..)
        | Reject::Duplicated(_)
        | Reject::DeclaredWrongCycles(..)
        | Reject::Resolve(_)
        | Reject::Expiry(_)) => fixed,
    }
}

fn public_description(reject: &PoolTransactionReject) -> &str {
    match reject {
        PoolTransactionReject::LowFeeRate(description)
        | PoolTransactionReject::ExceededMaximumAncestorsCount(description)
        | PoolTransactionReject::ExceededTransactionSizeLimit(description)
        | PoolTransactionReject::Full(description)
        | PoolTransactionReject::Duplicated(description)
        | PoolTransactionReject::Malformed(description)
        | PoolTransactionReject::DeclaredWrongCycles(description)
        | PoolTransactionReject::Resolve(description)
        | PoolTransactionReject::Verification(description)
        | PoolTransactionReject::Expiry(description)
        | PoolTransactionReject::RBFRejected(description)
        | PoolTransactionReject::Invalidated(description)
        | PoolTransactionReject::Internal(description) => description,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ComponentLimitKind {
    /// The bounded descendant closure mandated by RBF policy exceeded its
    /// public replacement-candidate limit.
    Replacement,
    /// A late-parent/capacity mutation would touch more accepted owners than
    /// one atomic authority Apply permits.
    Mutation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum MembershipReject {
    InputConflict(OutPoint),
    TooManyAncestors,
    ComponentLimit {
        kind: ComponentLimitKind,
        limit: usize,
    },
    NewUnconfirmedInput(OutPoint),
    InputFromDescendant(OutPoint),
    AncestorDescendantOverlap,
    DependencyOnVictim(OutPoint),
    InsufficientReplacementFee {
        actual: Capacity,
        required: Capacity,
    },
    ReplacementFeeOverflow,
    AggregateOverflow,
    CandidateEvicted {
        fee_rate: FeeRate,
    },
    CausalCycle(RawTxHash),
    MissingInputEvidence(OutPoint),
    MissingDependencyEvidence(OutPoint),
    MissingPoolOutput(OutPoint),
}

impl MembershipReject {
    /// Compile the exact final-membership outcome to the existing public
    /// tx-pool rejection domain.  This is deliberately exhaustive: adding a
    /// membership rule cannot compile until its RPC/relay semantics are
    /// chosen, and no caller may replace the reason with a coarse `Policy`
    /// class.
    pub(super) fn into_public(self) -> Reject {
        match self {
            // Preserve the existing non-RBF conflict surface: callers and
            // recent-reject storage observe the exact occupied outpoint.
            Self::InputConflict(out_point) => Reject::Resolve(OutPointError::Dead(out_point)),
            Self::TooManyAncestors => Reject::ExceededMaximumAncestorsCount,
            Self::ComponentLimit {
                kind: ComponentLimitKind::Replacement,
                limit,
            } => Reject::RBFRejected(format!(
                "Tx conflict with too many txs, conflict txs count: >= {}, expect <= {}",
                limit.saturating_add(1),
                limit,
            )),
            Self::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                limit,
            } => Reject::Full(format!(
                "pool mutation exceeds the per-transition limit of {limit}"
            )),
            Self::NewUnconfirmedInput(_) => {
                Reject::RBFRejected("new Tx contains unconfirmed inputs".to_owned())
            }
            Self::InputFromDescendant(_) => Reject::RBFRejected(
                "new Tx contains inputs in descendants of to be replaced Tx".to_owned(),
            ),
            Self::AncestorDescendantOverlap => Reject::RBFRejected(
                "Tx ancestors have common with conflict Tx descendants".to_owned(),
            ),
            Self::DependencyOnVictim(_) => {
                Reject::RBFRejected("new Tx contains cell deps from conflicts".to_owned())
            }
            Self::InsufficientReplacementFee { actual, required } => Reject::RBFRejected(format!(
                "Tx's current fee is {actual}, expect it to >= {required} to replace old txs"
            )),
            Self::ReplacementFeeOverflow => {
                Reject::RBFRejected("calculate_min_replace_fee failed".to_owned())
            }
            Self::AggregateOverflow => {
                Reject::Full("accepted pool aggregate capacity overflow".to_owned())
            }
            Self::CandidateEvicted { fee_rate } => {
                Reject::Full(format!("the fee_rate for this transaction is: {fee_rate}"))
            }
            Self::CausalCycle(hash) => Reject::Invalidated(format!(
                "candidate would create a causal cycle through {:?}",
                hash.0
            )),
            Self::MissingInputEvidence(out_point)
            | Self::MissingDependencyEvidence(out_point)
            | Self::MissingPoolOutput(out_point) => Reject::Resolve(OutPointError::Dead(out_point)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommittedPublicReject, ComponentLimitKind, MembershipReject};
    use crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
    use crate::error::Reject;
    use ckb_types::{
        core::{Capacity, FeeRate, error::OutPointError},
        packed::{Byte32, OutPoint},
    };

    fn out_point(byte: u8) -> OutPoint {
        OutPoint::new(Byte32::new([byte; 32]), 0)
    }

    #[test]
    fn membership_rejection_compiler_preserves_public_rule_semantics() {
        let occupied = out_point(1);
        assert!(matches!(
            MembershipReject::InputConflict(occupied.clone()).into_public(),
            Reject::Resolve(OutPointError::Dead(actual)) if actual == occupied
        ));

        assert!(matches!(
            MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Replacement,
                limit: 100,
            }
            .into_public(),
            Reject::RBFRejected(message)
                if message.contains(">= 101") && message.contains("<= 100")
        ));
        assert!(matches!(
            MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                limit: 100,
            }
            .into_public(),
            Reject::Full(message)
                if message == "pool mutation exceeds the per-transition limit of 100"
        ));

        let fee_rate = FeeRate::from_u64(42);
        assert!(matches!(
            MembershipReject::CandidateEvicted { fee_rate }.into_public(),
            Reject::Full(message)
                if message == format!("the fee_rate for this transaction is: {fee_rate}")
        ));
        let actual = Capacity::shannons(10);
        let required = Capacity::shannons(11);
        assert!(matches!(
            MembershipReject::InsufficientReplacementFee {
                actual,
                required,
            }
            .into_public(),
            Reject::RBFRejected(message)
                if message == format!(
                    "Tx's current fee is {actual}, expect it to >= {required} to replace old txs"
                )
        ));
    }

    #[test]
    fn committed_public_rejection_bounds_diagnostics_without_changing_policy() {
        let original = Reject::RBFRejected(
            "x".repeat(
                MAX_TX_POOL_REJECT_DESCRIPTION_BYTES
                    .checked_mul(2)
                    .expect("fixture length fits"),
            ),
        );
        let committed = CommittedPublicReject::new(original);
        assert!(matches!(committed.reject(), Reject::RBFRejected(_)));
        assert!(committed.description_bytes() <= MAX_TX_POOL_REJECT_DESCRIPTION_BYTES);
        assert!(!committed.is_malformed());
        assert!(committed.should_record());
        assert!(committed.relay_allowed());
    }

    #[test]
    fn committed_public_rejection_detaches_spare_string_capacity() {
        let mut diagnostic = String::with_capacity(
            MAX_TX_POOL_REJECT_DESCRIPTION_BYTES
                .checked_mul(4)
                .expect("fixture capacity fits"),
        );
        diagnostic.push_str("short transient diagnostic");
        let committed = CommittedPublicReject::new(Reject::Full(diagnostic));

        let Reject::Full(diagnostic) = committed.reject() else {
            panic!("the rejection variant must be preserved");
        };
        assert_eq!(diagnostic, "short transient diagnostic");
        assert!(diagnostic.capacity() <= MAX_TX_POOL_REJECT_DESCRIPTION_BYTES);
    }
}
