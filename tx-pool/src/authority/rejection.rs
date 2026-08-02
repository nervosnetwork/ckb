//! Exact, bounded reasons produced by final membership planning.
//!
//! These values are transaction outcomes, not authority faults.  Keeping the
//! closed domain outside both the membership projection and effect log lets a
//! single candidate-disposition Plan carry the same reason through owner
//! terminalization and committed publication without translating it to a
//! lossy class in between.

use super::ingress::DirectCommand;
use super::state::RawTxHash;
use super::state::{ApplySequence, ChainViewId};
use crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
use crate::error::Reject;
use ckb_jsonrpc_types::PoolTransactionReject;
use ckb_types::{
    core::{Capacity, FeeRate, TransactionView, error::OutPointError},
    packed::OutPoint,
};
use std::sync::Arc;

const MAX_DYNAMIC_REJECT_TEXT_BYTES: usize = MAX_TX_POOL_REJECT_DESCRIPTION_BYTES - 128;
pub(crate) const MAX_COMMIT_BAN_REASON_BYTES: usize = 1024;
pub(crate) const MAX_RECENT_REJECT_BYTES: usize = MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;

pub(crate) fn bounded_commit_ban_reason(reject: &Reject) -> String {
    let mut reason = format!("reject {reject}");
    if reason.len() > MAX_COMMIT_BAN_REASON_BYTES {
        let boundary = reason.floor_char_boundary(MAX_COMMIT_BAN_REASON_BYTES);
        reason.truncate(boundary);
    }
    reason
}

pub(crate) fn bounded_recent_reject(reject: &Reject) -> Reject {
    let rendered = reject.to_string();
    if rendered.len() <= MAX_RECENT_REJECT_BYTES {
        return reject.clone();
    }
    Reject::Malformed(
        "tx-pool".to_string(),
        format!(
            "rejection diagnostic omitted after exceeding {} bytes",
            MAX_RECENT_REJECT_BYTES
        ),
    )
}

#[derive(Debug)]
pub(crate) enum RecentRejectEncodingError {
    Json(serde_json::Error),
    FixedFallbackExceedsBound,
}

impl std::fmt::Display for RecentRejectEncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "recent-reject JSON encoding failed: {error}"),
            Self::FixedFallbackExceedsBound => {
                write!(formatter, "fixed recent-reject fallback exceeds its bound")
            }
        }
    }
}

impl std::error::Error for RecentRejectEncodingError {}

pub(crate) fn serialized_recent_reject(
    reject: &Reject,
) -> Result<String, RecentRejectEncodingError> {
    fn serialize(reject: Reject) -> Result<String, RecentRejectEncodingError> {
        let public: PoolTransactionReject = reject.into();
        serde_json::to_string(&public).map_err(RecentRejectEncodingError::Json)
    }

    let serialized = serialize(bounded_recent_reject(reject))?;
    if serialized.len() <= MAX_RECENT_REJECT_BYTES {
        return Ok(serialized);
    }
    let fallback = serialize(Reject::Malformed(
        "tx-pool rejection diagnostic omitted".to_string(),
        String::new(),
    ))?;
    if fallback.len() > MAX_RECENT_REJECT_BYTES {
        return Err(RecentRejectEncodingError::FixedFallbackExceedsBound);
    }
    Ok(fallback)
}

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

/// Exact pre-membership outcome shared by direct ingress, resolution, and
/// script verification. It retains the transaction needed by Local effect
/// publication while TestAccept may return the same reason without acquiring
/// an authority owner.
#[derive(Debug)]
pub(super) struct DirectTransactionRejection {
    tx: Arc<TransactionView>,
    command: DirectCommand,
    reason: CommittedPublicReject,
    validity: DirectRejectionValidity,
}

/// Exact authority evidence that keeps an owner-free rejection valid until a
/// Local publication or a TestAccept return. Stable ingress facts need no
/// chain fence. Chain-bound direct rejections use the existing Accepted source
/// cut because owner-free work is deliberately absent from the resident
/// dependency frontier; registering a synthetic consumer would turn this
/// read-only path into another lifecycle owner.
#[derive(Clone, Debug)]
pub(super) enum DirectRejectionValidity {
    Stable,
    AcceptedCut {
        view: ChainViewId,
        accepted: ApplySequence,
    },
}

impl DirectTransactionRejection {
    pub(super) fn stable(tx: Arc<TransactionView>, command: DirectCommand, reason: Reject) -> Self {
        Self {
            tx,
            command,
            reason: CommittedPublicReject::new(reason),
            validity: DirectRejectionValidity::Stable,
        }
    }

    pub(super) fn accepted_cut(
        tx: Arc<TransactionView>,
        command: DirectCommand,
        reason: Reject,
        view: ChainViewId,
        accepted: ApplySequence,
    ) -> Self {
        Self {
            tx,
            command,
            reason: CommittedPublicReject::new(reason),
            validity: DirectRejectionValidity::AcceptedCut { view, accepted },
        }
    }

    pub(super) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }

    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }

    pub(super) fn validity(&self) -> &DirectRejectionValidity {
        &self.validity
    }

    pub(super) fn command(&self) -> DirectCommand {
        self.command
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Arc<TransactionView>,
        DirectCommand,
        CommittedPublicReject,
        DirectRejectionValidity,
    ) {
        (self.tx, self.command, self.reason, self.validity)
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
    /// Whether the public outcome produced by this membership rejection is
    /// stable transaction evidence suitable for the recent-reject database.
    ///
    /// Keep this exhaustive and paired with [`Self::into_public`].  The three
    /// false cases compile to `Reject::Full`, which is transient node-local
    /// backpressure; every other membership failure compiles to a recordable
    /// public rejection.  Effect-batch construction uses this allocation-free
    /// classification while holding the authority guard.
    pub(super) const fn should_record_recent_reject(&self) -> bool {
        !matches!(
            self,
            Self::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                ..
            } | Self::AggregateOverflow
                | Self::CandidateEvicted { .. }
        )
    }

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
    fn membership_recent_reject_classification_matches_the_public_policy() {
        let cases = vec![
            MembershipReject::InputConflict(out_point(1)),
            MembershipReject::TooManyAncestors,
            MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Replacement,
                limit: 100,
            },
            MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                limit: 100,
            },
            MembershipReject::NewUnconfirmedInput(out_point(2)),
            MembershipReject::InputFromDescendant(out_point(3)),
            MembershipReject::AncestorDescendantOverlap,
            MembershipReject::DependencyOnVictim(out_point(4)),
            MembershipReject::InsufficientReplacementFee {
                actual: Capacity::shannons(10),
                required: Capacity::shannons(11),
            },
            MembershipReject::ReplacementFeeOverflow,
            MembershipReject::AggregateOverflow,
            MembershipReject::CandidateEvicted {
                fee_rate: FeeRate::from_u64(42),
            },
            MembershipReject::CausalCycle(super::RawTxHash(Byte32::new([5; 32]))),
            MembershipReject::MissingInputEvidence(out_point(6)),
            MembershipReject::MissingDependencyEvidence(out_point(7)),
            MembershipReject::MissingPoolOutput(out_point(8)),
        ];

        for reason in cases {
            assert_eq!(
                reason.should_record_recent_reject(),
                reason.clone().into_public().should_recorded(),
                "allocation-free effect classification drifted for {reason:?}"
            );
        }
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
