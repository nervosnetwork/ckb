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
pub(super) struct ApplySequence(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DependencyCut(pub(super) ApplySequence);

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

    pub(super) fn compute_attribution(self) -> ComputeAttribution {
        match self {
            Self::Remote(lease) => ComputeAttribution::Peer(lease.peer),
            Self::Proposal(_) | Self::Recovery(_) => ComputeAttribution::Trusted,
        }
    }
}

/// Attribution for one transient compute capability. Retained transaction
/// residency remains charged to immutable ingress independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeAttribution {
    Trusted,
    Peer(PeerIndex),
}

impl ComputeAttribution {
    pub(super) fn peer(self) -> Option<PeerIndex> {
        match self {
            Self::Trusted => None,
            Self::Peer(peer) => Some(peer),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DependencyKey {
    Cell(OutPoint),
    Header(Byte32),
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DependencyOrigin {
    Transaction(RawTxHash),
    BlockHeader(Byte32),
}

impl DependencyKey {
    pub(super) fn origin(&self) -> DependencyOrigin {
        match self {
            Self::Cell(out_point) => DependencyOrigin::Transaction(RawTxHash(out_point.tx_hash())),
            Self::Header(hash) => DependencyOrigin::BlockHeader(hash.clone()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct KnownDependencies(Arc<[DependencyKey]>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencySetError {
    Empty,
    TooMany,
    Arithmetic,
    Allocation,
}

impl KnownDependencies {
    fn canonicalize(
        mut keys: Vec<DependencyKey>,
        max: usize,
        allow_empty: bool,
    ) -> Result<Self, DependencySetError> {
        keys.sort_unstable();
        keys.dedup();
        if !allow_empty && keys.is_empty() {
            return Err(DependencySetError::Empty);
        }
        if keys.len() > max {
            return Err(DependencySetError::TooMany);
        }
        Ok(Self(keys.into()))
    }

    pub(super) fn from_transaction(tx: &TransactionView) -> Result<Self, DependencySetError> {
        let capacity = tx
            .inputs()
            .len()
            .checked_add(tx.cell_deps().len())
            .and_then(|count| count.checked_add(tx.header_deps().len()))
            .ok_or(DependencySetError::Arithmetic)?;
        let mut keys = Vec::new();
        keys.try_reserve(capacity)
            .map_err(|_| DependencySetError::Allocation)?;
        keys.extend(tx.input_pts_iter().map(DependencyKey::Cell));
        keys.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| DependencyKey::Cell(dependency.out_point())),
        );
        keys.extend(tx.header_deps().into_iter().map(DependencyKey::Header));
        Self::canonicalize(keys, capacity, true)
    }

    pub(super) fn from_footprint(
        footprint: &ExpandedFootprint,
        max: usize,
    ) -> Result<Self, DependencySetError> {
        let mut keys = Vec::new();
        keys.try_reserve(footprint.edge_count())
            .map_err(|_| DependencySetError::Allocation)?;
        keys.extend(footprint.inputs().iter().cloned().map(DependencyKey::Cell));
        keys.extend(
            footprint
                .dependencies()
                .iter()
                .cloned()
                .map(DependencyKey::Cell),
        );
        keys.extend(
            footprint
                .header_dependencies()
                .iter()
                .cloned()
                .map(DependencyKey::Header),
        );
        Self::canonicalize(keys, max, true)
    }

    pub(super) fn with_missing(
        &self,
        missing: &MissingDependencies,
        max: usize,
    ) -> Result<Self, DependencySetError> {
        let capacity = self
            .len()
            .checked_add(missing.len())
            .ok_or(DependencySetError::Arithmetic)?;
        let mut keys = Vec::new();
        keys.try_reserve(capacity)
            .map_err(|_| DependencySetError::Allocation)?;
        keys.extend(self.keys().iter().cloned());
        keys.extend(missing.keys().iter().cloned());
        Self::canonicalize(keys, max, true)
    }

    pub(super) fn keys(&self) -> &[DependencyKey] {
        self.0.as_ref()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn contains(&self, key: &DependencyKey) -> bool {
        self.0.binary_search(key).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MissingDependencies(KnownDependencies);

impl MissingDependencies {
    pub(super) fn new(keys: Vec<DependencyKey>, max: usize) -> Result<Self, DependencySetError> {
        if keys.len() > max {
            return Err(DependencySetError::TooMany);
        }
        KnownDependencies::canonicalize(keys, max, false).map(Self)
    }

    pub(super) fn keys(&self) -> &[DependencyKey] {
        self.0.keys()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
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
    identity: TxIdentity,
    pub(super) footprint: ExpandedFootprint,
    dependencies: KnownDependencies,
    fee: Capacity,
    serialized_bytes: usize,
    resolved_resident_bytes: usize,
    chain_inputs: Vec<OutPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvidenceError {
    Footprint(FootprintError),
    DependencySet(DependencySetError),
    NotAnInput,
    ResidentBelowSerialized,
}

impl ResolvedPayload {
    /// Constructs one resolution fact from the exact transaction. Positive
    /// chain inputs and expanded dependencies cannot be paired with a second
    /// transaction identity.
    pub(super) fn from_resolution(
        _seal: super::work::ResolutionSeal,
        tx: &TransactionView,
        expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
        chain_inputs: Vec<OutPoint>,
    ) -> Result<Self, InputEvidenceError> {
        Self::from_transaction_parts(
            tx,
            expanded_dependencies,
            max_edges,
            fee,
            resolved_resident_bytes,
            chain_inputs,
        )
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        tx: &TransactionView,
        expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
        chain_inputs: Vec<OutPoint>,
    ) -> Result<Self, InputEvidenceError> {
        Self::from_transaction_parts(
            tx,
            expanded_dependencies,
            max_edges,
            fee,
            resolved_resident_bytes,
            chain_inputs,
        )
    }

    fn from_transaction_parts(
        tx: &TransactionView,
        expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
        mut chain_inputs: Vec<OutPoint>,
    ) -> Result<Self, InputEvidenceError> {
        let footprint = ExpandedFootprint::from_transaction(tx, expanded_dependencies, max_edges)
            .map_err(InputEvidenceError::Footprint)?;
        let dependencies = KnownDependencies::from_footprint(&footprint, max_edges)
            .map_err(InputEvidenceError::DependencySet)?;
        let serialized_bytes = tx.data().total_size();
        if resolved_resident_bytes < serialized_bytes {
            return Err(InputEvidenceError::ResidentBelowSerialized);
        }
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        if chain_inputs
            .iter()
            .any(|input| footprint.inputs.binary_search(input).is_err())
        {
            return Err(InputEvidenceError::NotAnInput);
        }
        Ok(Self {
            identity: TxIdentity::from_transaction(tx),
            footprint,
            dependencies,
            fee,
            serialized_bytes,
            resolved_resident_bytes,
            chain_inputs,
        })
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    pub(super) fn identity(&self) -> &TxIdentity {
        &self.identity
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        &self.dependencies
    }

    pub(super) fn fee(&self) -> Capacity {
        self.fee
    }

    pub(super) fn serialized_bytes(&self) -> usize {
        self.serialized_bytes
    }

    pub(super) fn resolved_resident_bytes(&self) -> usize {
        self.resolved_resident_bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedFacts {
    chain_epoch: ChainEpoch,
    dependency_cut: DependencyCut,
    payload: Arc<ResolvedPayload>,
    verify_class: VerifyCycleClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VerifiedFacts {
    witness: WitnessTxHash,
    chain_epoch: ChainEpoch,
    dependency_cut: DependencyCut,
    payload: Arc<ResolvedPayload>,
    metrics: CandidateMetrics,
}

impl ResolvedFacts {
    pub(super) fn from_resolution(
        _seal: super::work::ResolutionSeal,
        chain_epoch: ChainEpoch,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self {
            chain_epoch,
            dependency_cut,
            payload,
            verify_class,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        chain_epoch: ChainEpoch,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self {
            chain_epoch,
            dependency_cut,
            payload,
            verify_class,
        }
    }

    pub(super) fn chain_epoch(&self) -> ChainEpoch {
        self.chain_epoch
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        &self.payload
    }

    pub(super) fn verify_class(&self) -> VerifyCycleClass {
        self.verify_class
    }

    pub(super) fn into_verification_parts(
        self,
        _seal: super::work::VerificationSeal,
    ) -> (
        ChainEpoch,
        DependencyCut,
        Arc<ResolvedPayload>,
        VerifyCycleClass,
    ) {
        (
            self.chain_epoch,
            self.dependency_cut,
            self.payload,
            self.verify_class,
        )
    }
}

impl VerifiedFacts {
    pub(super) fn from_verification(
        _seal: super::work::VerificationSeal,
        witness: WitnessTxHash,
        chain_epoch: ChainEpoch,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        Self {
            witness,
            chain_epoch,
            dependency_cut,
            payload,
            metrics,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        witness: WitnessTxHash,
        chain_epoch: ChainEpoch,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        Self {
            witness,
            chain_epoch,
            dependency_cut,
            payload,
            metrics,
        }
    }

    pub(super) fn witness(&self) -> &WitnessTxHash {
        &self.witness
    }

    pub(super) fn chain_epoch(&self) -> ChainEpoch {
        self.chain_epoch
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        &self.payload
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VerifyCycleClass {
    #[default]
    Small,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerifyCapability {
    Any,
    SmallCycleOnly,
}

impl VerifyCapability {
    pub(super) fn permits(self, class: VerifyCycleClass) -> bool {
        match self {
            Self::Any => true,
            Self::SmallCycleOnly => class == VerifyCycleClass::Small,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkPermit {
    ResolveOnly,
    VerifyOnly(VerifyCapability),
    ResolveThenVerify(VerifyCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeGrant {
    pub(super) max_resident_bytes: usize,
    pub(super) max_edges: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QueuedWork {
    Resolve,
    Verify(ResolvedFacts),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActiveWork {
    pub(super) lease: ComputeLeaseId,
    pub(super) permit: WorkPermit,
    pub(super) grant: ComputeGrant,
    pub(super) attribution: ComputeAttribution,
    pub(super) dependency_cut: DependencyCut,
    pub(super) dependencies: KnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ObservedDependencies {
    dependency_cut: DependencyCut,
    observed: KnownDependencies,
    retained: KnownDependencies,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyObservationError {
    EmptyOrDuplicate,
}

impl ObservedDependencies {
    pub(super) fn from_missing(
        dependencies: &MissingDependencies,
        retained: KnownDependencies,
        dependency_cut: DependencyCut,
    ) -> Self {
        Self {
            dependency_cut,
            observed: dependencies.0.clone(),
            retained,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        dependencies: Vec<DependencyKey>,
        dependency_cut: DependencyCut,
    ) -> Result<Self, DependencyObservationError> {
        let max = dependencies.len();
        let dependencies = KnownDependencies::canonicalize(dependencies, max, false)
            .map_err(|_| DependencyObservationError::EmptyOrDuplicate)?;
        Ok(Self {
            dependency_cut,
            observed: dependencies.clone(),
            retained: dependencies,
        })
    }

    pub(super) fn contains(&self, key: &DependencyKey) -> bool {
        self.observed.contains(key)
    }

    pub(super) fn keys(&self) -> impl ExactSizeIterator<Item = &DependencyKey> {
        self.observed.keys().iter()
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }

    pub(super) fn retained(&self) -> &KnownDependencies {
        &self.retained
    }

    pub(super) fn len(&self) -> usize {
        self.observed.len()
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
    UnavailableDependency,
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
pub(super) struct AdmissionBasis {
    declared_dependencies: KnownDependencies,
    original_charge: ResourceVector,
}

impl AdmissionBasis {
    pub(super) fn new(
        declared_dependencies: KnownDependencies,
        original_charge: ResourceVector,
    ) -> Self {
        Self {
            declared_dependencies,
            original_charge,
        }
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        &self.declared_dependencies
    }

    pub(super) fn charge(&self) -> ResourceVector {
        self.original_charge
    }
}

#[derive(Clone, Debug)]
pub(super) struct PreAcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) basis: AdmissionBasis,
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
    pub(super) fn dependencies(&self) -> &KnownDependencies {
        match &self.phase {
            PreAcceptedPhase::Queued(QueuedWork::Resolve)
            | PreAcceptedPhase::Computed(
                ComputedOutcome::Rejected(_)
                | ComputedOutcome::BudgetDenied
                | ComputedOutcome::InternalFailure,
            ) => self.basis.dependencies(),
            PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => {
                resolved.payload().dependencies()
            }
            PreAcceptedPhase::Computing(active) => &active.dependencies,
            PreAcceptedPhase::Waiting(WaitCondition::Missing(observed))
            | PreAcceptedPhase::Waiting(WaitCondition::Conflict(observed)) => observed.retained(),
            PreAcceptedPhase::Computed(ComputedOutcome::Verified(verified)) => {
                verified.payload().dependencies()
            }
        }
    }

    pub(super) fn retained_charge(
        &self,
        resident_bytes: usize,
        dependencies: &KnownDependencies,
    ) -> ResourceVector {
        ResourceVector::new(
            1,
            self.basis.charge().bytes.max(resident_bytes),
            self.basis.charge().edges.max(dependencies.len()),
            0,
        )
    }

    pub(super) fn original_charge(&self) -> ResourceVector {
        self.basis.charge()
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        let compute_peer = match &self.phase {
            PreAcceptedPhase::Computing(active) => active.attribution.peer(),
            PreAcceptedPhase::Queued(_)
            | PreAcceptedPhase::Waiting(_)
            | PreAcceptedPhase::Computed(_) => None,
        };
        ChargeRecord::PreAccepted {
            resources: self.charge,
            // A trust/context promotion must never erase the peer-origin
            // DoS charge. Accepted membership deliberately changes to a
            // distinct global pool charge instead of hand-clearing peer
            // counters.
            residency_peer: self.record.ingress.peer(),
            compute_peer,
        }
    }
}

impl AcceptedEntry {
    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord::Accepted(AcceptedResources::one(self.verified.metrics().cost))
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
    pub(super) dependencies: KnownDependencies,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AdmissionValidationError {
    EmptyTransaction,
    AttributionMismatch,
    ResourceArithmetic,
    ResourceAllocation,
}

impl ValidatedAdmission {
    pub(super) fn remote(
        tx: TransactionView,
        peer: PeerIndex,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Peer(peer),
            PayloadBlame::Peer(peer),
            AdmissionClass::Remote(RemoteLease { peer }),
        )
    }

    pub(super) fn proposal(
        tx: TransactionView,
        context: ProposalContextId,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Trusted,
            PayloadBlame::None,
            AdmissionClass::Proposal(ProposalLease { context }),
        )
    }

    pub(super) fn recovery(
        tx: TransactionView,
        epoch: ChainEpoch,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            IngressAttribution::Trusted,
            PayloadBlame::None,
            AdmissionClass::Recovery(RecoveryLease { epoch }),
        )
    }

    fn new(
        tx: TransactionView,
        ingress: IngressAttribution,
        blame: PayloadBlame,
        class: AdmissionClass,
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
        let raw_edges = tx
            .inputs()
            .len()
            .checked_add(tx.cell_deps().len())
            .and_then(|count| count.checked_add(tx.header_deps().len()))
            .ok_or(AdmissionValidationError::ResourceArithmetic)?;
        let dependencies =
            KnownDependencies::from_transaction(&tx).map_err(|error| match error {
                DependencySetError::Arithmetic => AdmissionValidationError::ResourceArithmetic,
                DependencySetError::Allocation => AdmissionValidationError::ResourceAllocation,
                DependencySetError::Empty | DependencySetError::TooMany => {
                    AdmissionValidationError::ResourceArithmetic
                }
            })?;
        // The reverse projection is a canonical set, but ingress accounting
        // deliberately charges every encoded edge. Duplicate declarations do
        // not buy an attacker extra pre-pool residency for free.
        let charge = ResourceVector::new(1, bytes, raw_edges, 0);
        Ok(Self {
            identity: TxIdentity::from_transaction(&tx),
            tx: Arc::new(tx),
            ingress,
            blame,
            class,
            dependencies,
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
