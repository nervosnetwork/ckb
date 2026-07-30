use super::resources::{ChargeRecord, ResourceVector};
use ckb_network::PeerIndex;
use ckb_types::{
    core::TransactionView,
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::sync::Arc;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct RawTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct WitnessTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct ProposalId(pub(super) ProposalShortId);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryVersion(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ComputeLeaseId(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ChainEpoch(pub(super) u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DependencyEpoch(pub(super) u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ApplySequence(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Arrival(pub(super) u128);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TxIdentity {
    pub(super) raw: RawTxHash,
    pub(super) witness: WitnessTxHash,
    pub(super) proposal: ProposalId,
}

impl TxIdentity {
    pub(super) fn from_transaction(tx: &TransactionView) -> Self {
        Self {
            raw: RawTxHash(tx.hash()),
            witness: WitnessTxHash(tx.witness_hash()),
            proposal: ProposalId(tx.proposal_short_id()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IngressAttribution {
    Peer(PeerIndex),
    Trusted,
}

impl IngressAttribution {
    pub(super) fn peer(self) -> Option<PeerIndex> {
        match self {
            Self::Peer(peer) => Some(peer),
            Self::Trusted => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PayloadBlame {
    Peer(PeerIndex),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProposalContextId(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteLease {
    pub(super) peer: PeerIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProposalLease {
    pub(super) context: ProposalContextId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveryLease {
    pub(super) epoch: ChainEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionClass {
    Remote(RemoteLease),
    Proposal(ProposalLease),
    Recovery(RecoveryLease),
}

impl AdmissionClass {
    fn initial_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(lease) => Some(lease.peer),
            Self::Proposal(_) | Self::Recovery(_) => None,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum DependencyKey {
    Cell(OutPoint),
    Header(Byte32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedFacts {
    pub(super) chain_epoch: ChainEpoch,
    pub(super) dependency_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VerifiedFacts {
    pub(super) witness: WitnessTxHash,
    pub(super) chain_epoch: ChainEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkPermit {
    ResolveOnly,
    VerifyOnly,
    ResolveThenVerify,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QueuedWork {
    Resolve,
    Verify(ResolvedFacts),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActiveWork {
    pub(super) lease: ComputeLeaseId,
    pub(super) permit: WorkPermit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ObservedDependency {
    pub(super) key: DependencyKey,
    pub(super) epoch: DependencyEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ObservedDependencies(Vec<ObservedDependency>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyObservationError {
    Empty,
}

impl ObservedDependencies {
    pub(super) fn new(
        dependencies: Vec<ObservedDependency>,
    ) -> Result<Self, DependencyObservationError> {
        if dependencies.is_empty() {
            Err(DependencyObservationError::Empty)
        } else {
            Ok(Self(dependencies))
        }
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WaitCondition {
    Missing(ObservedDependencies),
    Conflict(ObservedDependencies),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RejectionKind {
    Verification,
    Policy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComputedOutcome {
    Verified(VerifiedFacts),
    Rejected(RejectionKind),
    BudgetDenied,
    InternalFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PreAcceptedPhase {
    Queued(QueuedWork),
    Computing(ActiveWork),
    Waiting(WaitCondition),
    Computed(ComputedOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AcceptedStatus {
    Pending,
    Gap,
    Proposed,
}

#[derive(Clone, Debug)]
pub(super) struct TxRecord {
    pub(super) tx: Arc<TransactionView>,
    pub(super) identity: TxIdentity,
    pub(super) ingress: IngressAttribution,
    pub(super) blame: PayloadBlame,
    pub(super) class: AdmissionClass,
    pub(super) version: EntryVersion,
    pub(super) arrival: Arrival,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug)]
pub(super) struct PreAcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) phase: PreAcceptedPhase,
}

#[derive(Clone, Debug)]
pub(super) struct AcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) status: AcceptedStatus,
}

#[derive(Clone, Debug)]
pub(super) enum OwnedTx {
    PreAccepted(PreAcceptedEntry),
    Accepted(AcceptedEntry),
}

impl OwnedTx {
    pub(super) fn record(&self) -> &TxRecord {
        match self {
            Self::PreAccepted(entry) => &entry.record,
            Self::Accepted(entry) => &entry.record,
        }
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord {
            resources: self.record().charge,
            // A trust/context promotion must never erase the peer-origin DoS
            // charge. Ingress is immutable; AdmissionClass is intentionally
            // allowed to change while the same owner continues computing.
            peer: self.record().ingress.peer(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ValidatedAdmission {
    pub(super) tx: Arc<TransactionView>,
    pub(super) identity: TxIdentity,
    pub(super) ingress: IngressAttribution,
    pub(super) blame: PayloadBlame,
    pub(super) class: AdmissionClass,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AdmissionValidationError {
    EmptyTransaction,
    AttributionMismatch,
}

impl ValidatedAdmission {
    pub(super) fn remote(
        tx: TransactionView,
        peer: PeerIndex,
        edges: usize,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Peer(peer),
            PayloadBlame::Peer(peer),
            AdmissionClass::Remote(RemoteLease { peer }),
            edges,
        )
    }

    pub(super) fn proposal(
        tx: TransactionView,
        context: ProposalContextId,
        edges: usize,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Trusted,
            PayloadBlame::None,
            AdmissionClass::Proposal(ProposalLease { context }),
            edges,
        )
    }

    pub(super) fn recovery(
        tx: TransactionView,
        epoch: ChainEpoch,
        edges: usize,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Trusted,
            PayloadBlame::None,
            AdmissionClass::Recovery(RecoveryLease { epoch }),
            edges,
        )
    }

    fn new(
        tx: TransactionView,
        ingress: IngressAttribution,
        blame: PayloadBlame,
        class: AdmissionClass,
        edges: usize,
    ) -> Result<Self, AdmissionValidationError> {
        let bytes = tx.data().total_size();
        if bytes == 0 {
            return Err(AdmissionValidationError::EmptyTransaction);
        }
        let source_peer = class.initial_peer();
        let ingress_peer = match ingress {
            IngressAttribution::Peer(peer) => Some(peer),
            IngressAttribution::Trusted => None,
        };
        let blame_peer = match blame {
            PayloadBlame::Peer(peer) => Some(peer),
            PayloadBlame::None => None,
        };
        if source_peer != ingress_peer || source_peer != blame_peer {
            return Err(AdmissionValidationError::AttributionMismatch);
        }
        let charge = ResourceVector::new(1, bytes, edges, 0);
        Ok(Self {
            identity: TxIdentity::from_transaction(&tx),
            tx: Arc::new(tx),
            ingress,
            blame,
            class,
            charge,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuthorityClocks {
    pub(super) next_version: EntryVersion,
    pub(super) next_lease: ComputeLeaseId,
    pub(super) next_arrival: Arrival,
    pub(super) next_sequence: ApplySequence,
}

impl AuthorityClocks {
    pub(super) const fn first() -> Self {
        Self {
            next_version: EntryVersion(1),
            next_lease: ComputeLeaseId(1),
            next_arrival: Arrival(0),
            next_sequence: ApplySequence(1),
        }
    }
}
