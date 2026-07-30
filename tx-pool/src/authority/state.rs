use super::resources::{AcceptedCost, AcceptedResources, ChargeRecord, ResourceVector};
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Capacity, TransactionView},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::sync::Arc;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
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
pub(super) struct ExpandedFootprint {
    inputs: Vec<OutPoint>,
    dependencies: Vec<OutPoint>,
    header_dependencies: Vec<Byte32>,
    edge_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FootprintError {
    DuplicateInput,
    TooManyEdges,
    Arithmetic,
}

impl ExpandedFootprint {
    pub(super) fn from_transaction(
        tx: &TransactionView,
        mut expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
    ) -> Result<Self, FootprintError> {
        let mut inputs = tx.input_pts_iter().collect::<Vec<_>>();
        let input_count = inputs.len();
        inputs.sort_unstable();
        inputs.dedup();
        if inputs.len() != input_count {
            return Err(FootprintError::DuplicateInput);
        }

        expanded_dependencies.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| dependency.out_point()),
        );
        expanded_dependencies.sort_unstable();
        expanded_dependencies.dedup();
        expanded_dependencies.retain(|dependency| inputs.binary_search(dependency).is_err());
        let mut header_dependencies = tx.header_deps().into_iter().collect::<Vec<_>>();
        header_dependencies.sort_unstable();
        header_dependencies.dedup();
        let edge_count = inputs
            .len()
            .checked_add(expanded_dependencies.len())
            .and_then(|count| count.checked_add(header_dependencies.len()))
            .ok_or(FootprintError::Arithmetic)?;
        if edge_count > max_edges {
            return Err(FootprintError::TooManyEdges);
        }
        Ok(Self {
            inputs,
            dependencies: expanded_dependencies,
            header_dependencies,
            edge_count,
        })
    }

    pub(super) fn inputs(&self) -> &[OutPoint] {
        &self.inputs
    }

    pub(super) fn dependencies(&self) -> &[OutPoint] {
        &self.dependencies
    }

    pub(super) fn header_dependencies(&self) -> &[Byte32] {
        &self.header_dependencies
    }

    pub(super) fn edge_count(&self) -> usize {
        self.edge_count
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CandidateMetrics {
    pub(super) fee: Capacity,
    pub(super) cost: AcceptedCost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedPayload {
    pub(super) footprint: ExpandedFootprint,
    pub(super) metrics: CandidateMetrics,
    chain_inputs: Vec<OutPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvidenceError {
    NotAnInput,
}

impl ResolvedPayload {
    /// Captures positive chain-input evidence from the same snapshot that
    /// produced `footprint`. Pool-produced inputs are intentionally absent.
    pub(super) fn new(
        footprint: ExpandedFootprint,
        metrics: CandidateMetrics,
        mut chain_inputs: Vec<OutPoint>,
    ) -> Result<Self, InputEvidenceError> {
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        if chain_inputs
            .iter()
            .any(|input| footprint.inputs.binary_search(input).is_err())
        {
            return Err(InputEvidenceError::NotAnInput);
        }
        Ok(Self {
            footprint,
            metrics,
            chain_inputs,
        })
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedFacts {
    pub(super) chain_epoch: ChainEpoch,
    pub(super) payload: Arc<ResolvedPayload>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VerifiedFacts {
    pub(super) witness: WitnessTxHash,
    pub(super) chain_epoch: ChainEpoch,
    pub(super) payload: Arc<ResolvedPayload>,
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
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
}

#[derive(Clone, Debug)]
pub(super) struct PreAcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) phase: PreAcceptedPhase,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug)]
pub(super) struct AcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) status: AcceptedStatus,
    pub(super) verified: VerifiedFacts,
}

#[derive(Clone, Debug)]
pub(super) enum OwnedTx {
    PreAccepted(PreAcceptedEntry),
    Accepted(AcceptedEntry),
}

impl PreAcceptedEntry {
    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord::PreAccepted {
            resources: self.charge,
            // A trust/context promotion must never erase the peer-origin
            // DoS charge. Accepted membership deliberately changes to a
            // distinct global pool charge instead of hand-clearing peer
            // counters.
            peer: self.record.ingress.peer(),
        }
    }
}

impl AcceptedEntry {
    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord::Accepted(AcceptedResources::one(self.verified.payload.metrics.cost))
    }
}

impl OwnedTx {
    pub(super) fn record(&self) -> &TxRecord {
        match self {
            Self::PreAccepted(entry) => &entry.record,
            Self::Accepted(entry) => &entry.record,
        }
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        match self {
            Self::PreAccepted(entry) => entry.charge_record(),
            Self::Accepted(entry) => entry.charge_record(),
        }
    }

    pub(super) fn preaccepted_charge(&self) -> Option<ResourceVector> {
        match self {
            Self::PreAccepted(entry) => Some(entry.charge),
            Self::Accepted(_) => None,
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
