//! Exact, bounded reasons produced by final membership planning.
//!
//! These values are transaction outcomes, not authority faults.  Keeping the
//! closed domain outside both the membership projection and effect log lets a
//! single candidate-disposition Plan carry the same reason through owner
//! terminalization and committed publication without translating it to a
//! lossy class in between.

use super::ingress::DirectCommand;
use super::resolver::AcceptedOverlay;
use super::state::ChainViewId;
use super::state::RawTxHash;
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
    publish_negative_relay_terminal: bool,
    description_bytes: usize,
}

impl std::fmt::Debug for CommittedPublicReject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedPublicReject")
            .field("reject", &self.reject)
            .field("malformed", &self.malformed)
            .field("recordable", &self.recordable)
            .field(
                "publish_negative_relay_terminal",
                &self.publish_negative_relay_terminal,
            )
            .finish()
    }
}

impl PartialEq for CommittedPublicReject {
    fn eq(&self, other: &Self) -> bool {
        PoolTransactionReject::from(self.reject.clone())
            == PoolTransactionReject::from(other.reject.clone())
            && self.malformed == other.malformed
            && self.recordable == other.recordable
            && self.publish_negative_relay_terminal == other.publish_negative_relay_terminal
    }
}

impl Eq for CommittedPublicReject {}

pub(super) fn duplicate_inputs_reject() -> Reject {
    Reject::Malformed(
        "DuplicateInput".to_owned(),
        "transaction contains duplicate inputs".to_owned(),
    )
}

impl CommittedPublicReject {
    pub(super) fn new(reject: Reject) -> Self {
        let malformed = reject.is_malformed_tx();
        let recordable = reject.should_recorded();
        // `Reject::is_allowed_relay` is a historical public name. Its sole
        // current production consumer emits a negative terminal into sync's
        // pending-relay projection; it never positively disseminates the
        // rejected transaction. Keep the compatibility method at the shared
        // type boundary while naming the owned effect precisely here.
        let publish_negative_relay_terminal = reject.is_allowed_relay();
        let reject = bound_reject_diagnostic(reject);
        let description_bytes =
            public_description(&PoolTransactionReject::from(reject.clone())).len();
        Self {
            reject,
            malformed,
            recordable,
            publish_negative_relay_terminal,
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

    pub(super) const fn publish_negative_relay_terminal(&self) -> bool {
        self.publish_negative_relay_terminal
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
/// chain fence. Accepted-dependent work carries only its exact bounded read
/// receipt; owner-free work deliberately remains absent from the resident
/// dependency frontier and therefore creates no synthetic lifecycle owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DirectRejectionValidity {
    Stable,
    AcceptedReads {
        view: ChainViewId,
        reads: AcceptedOverlay,
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

    pub(super) fn accepted_reads(
        tx: Arc<TransactionView>,
        command: DirectCommand,
        reason: Reject,
        view: ChainViewId,
        reads: AcceptedOverlay,
    ) -> Self {
        Self {
            tx,
            command,
            reason: CommittedPublicReject::new(reason),
            validity: DirectRejectionValidity::AcceptedReads { view, reads },
        }
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
        // journal, even when its Display text happens to be short. Preserve
        // the established public shape by detaching the direct inner
        // diagnostic and letting the typed kind add its prefix exactly once.
        // Detaching `error.to_string()` would duplicate the top-level kind;
        // detaching `root_cause()` would instead erase any meaningful
        // intermediate error context.
        Reject::Verification(error) => {
            let kind = error.kind();
            let detached = match error.cause() {
                Some(cause) => kind.other(bounded_text(
                    cause.to_string(),
                    MAX_DYNAMIC_REJECT_TEXT_BYTES,
                )),
                None => kind.into(),
            };
            Reject::Verification(detached)
        }
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
        | Reject::ExcessiveVerifyTime
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
        | PoolTransactionReject::ExcessiveVerifyTime(description)
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
#[path = "tests/rejection_unit.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/support/rejection.rs"]
mod test_support;
