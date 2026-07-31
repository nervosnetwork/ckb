use super::chain::{
    AcceptedProof, CellContentReceipt, CellLocationReceipt, ProposalContextReceipt, ScriptReceipt,
    ValidationRulesId, VerificationContextReceipt,
};
use super::resources::{AcceptedCost, AcceptedResources, ChargeRecord, ResourceVector};
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::sync::Arc;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RawTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct WitnessTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ProposalId(pub(super) ProposalShortId);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryVersion(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ComputeLeaseId(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ChainRevision(pub(super) u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PoolGeneration(pub(super) u64);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ChainTipHash(pub(super) Byte32);

/// Exact authority view used for chain-plan OCC. `revision` distinguishes a
/// T -> T' -> T event sequence; `tip` identifies the immutable chain state
/// whose positive cell evidence may be reused.
#[derive(Debug, Hash, PartialEq, Eq)]
struct ChainViewIdentity {
    revision: ChainRevision,
    tip: ChainTipHash,
}

/// Cheap immutable identity shared by work and proof receipts. A chain
/// transition allocates one identity; per-entry/work clones do not duplicate
/// the 32-byte tip hash.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct ChainViewId(Arc<ChainViewIdentity>);

impl ChainViewId {
    pub(super) fn new(revision: ChainRevision, tip: Byte32) -> Self {
        Self(Arc::new(ChainViewIdentity {
            revision,
            tip: ChainTipHash(tip),
        }))
    }

    pub(super) fn initial() -> Self {
        Self::new(ChainRevision(0), Byte32::zero())
    }

    pub(super) fn revision(&self) -> ChainRevision {
        self.0.revision
    }

    pub(super) fn tip(&self) -> &ChainTipHash {
        &self.0.tip
    }

    pub(super) fn has_same_chain_state(&self, other: &Self) -> bool {
        self.0.tip == other.0.tip
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ApplySequence(pub(super) u128);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DependencyCut(pub(super) ApplySequence);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Arrival(pub(super) u128);

/// Wall-clock metadata captured when a transaction enters accepted
/// membership. It is observable through RPC, but never participates in pool
/// ordering, validation, budgets or replacement policy.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AcceptedAtMillis(pub(super) u64);

impl AcceptedAtMillis {
    #[cfg(test)]
    pub(super) const FOUNDATION: Self = Self(0);
}

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
pub(super) struct ProposalContextId(pub(super) u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RemoteDeadline(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteResidencyLease {
    pub(super) peer: PeerIndex,
    pub(super) expires_at: RemoteDeadline,
}

impl RemoteResidencyLease {
    pub(super) const fn new(peer: PeerIndex, expires_at: RemoteDeadline) -> Self {
        Self { peer, expires_at }
    }

    #[cfg(test)]
    pub(super) const fn for_foundation(peer: PeerIndex) -> Self {
        Self::new(peer, RemoteDeadline(u64::MAX))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProposalLease {
    pub(super) context: ProposalContextId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveryLease {
    pub(super) generation: PoolGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemotePayloadOrigin {
    IngressPeer,
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteBase {
    pub(super) residency: RemoteResidencyLease,
    pub(super) payload: RemotePayloadOrigin,
}

impl RemoteBase {
    pub(super) const fn ingress(residency: RemoteResidencyLease) -> Self {
        Self {
            residency,
            payload: RemotePayloadOrigin::IngressPeer,
        }
    }

    pub(super) const fn with_trusted_payload(self) -> Self {
        Self {
            residency: self.residency,
            payload: RemotePayloadOrigin::Trusted,
        }
    }

    pub(super) const fn blame_peer(self) -> Option<PeerIndex> {
        match self.payload {
            RemotePayloadOrigin::IngressPeer => Some(self.residency.peer),
            RemotePayloadOrigin::Trusted => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProposalBase {
    Trusted,
    Remote(RemoteBase),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreAcceptedSource {
    Remote(RemoteBase),
    Proposal {
        lease: ProposalLease,
        base: ProposalBase,
    },
    Recovery(RecoveryLease),
}

impl PreAcceptedSource {
    pub(super) const fn ingress_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(remote) => Some(remote.residency.peer),
            Self::Proposal {
                base: ProposalBase::Remote(remote),
                ..
            } => Some(remote.residency.peer),
            Self::Proposal {
                base: ProposalBase::Trusted,
                ..
            }
            | Self::Recovery(_) => None,
        }
    }

    pub(super) const fn payload_blame_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(remote)
            | Self::Proposal {
                base: ProposalBase::Remote(remote),
                ..
            } => remote.blame_peer(),
            Self::Proposal {
                base: ProposalBase::Trusted,
                ..
            }
            | Self::Recovery(_) => None,
        }
    }

    pub(super) const fn active_remote_deadline(self) -> Option<RemoteDeadline> {
        match self {
            Self::Remote(remote) => Some(remote.residency.expires_at),
            Self::Proposal { .. } | Self::Recovery(_) => None,
        }
    }

    pub(super) fn compute_attribution(self) -> ComputeAttribution {
        match self {
            Self::Remote(remote) => ComputeAttribution::Peer(remote.residency.peer),
            Self::Proposal { .. } | Self::Recovery(_) => ComputeAttribution::Trusted,
        }
    }

    pub(super) const fn accepted_provenance(self) -> AcceptedProvenance {
        match self {
            Self::Remote(remote)
            | Self::Proposal {
                base: ProposalBase::Remote(remote),
                ..
            } => AcceptedProvenance::Peer {
                ingress: remote.residency.peer,
                payload: remote.payload,
            },
            Self::Proposal {
                base: ProposalBase::Trusted,
                ..
            }
            | Self::Recovery(_) => AcceptedProvenance::Trusted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AcceptedProvenance {
    Trusted,
    Peer {
        ingress: PeerIndex,
        payload: RemotePayloadOrigin,
    },
}

impl AcceptedProvenance {
    pub(super) const fn ingress_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Trusted => None,
            Self::Peer { ingress, .. } => Some(ingress),
        }
    }

    pub(super) const fn payload_blame_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Peer {
                ingress,
                payload: RemotePayloadOrigin::IngressPeer,
            } => Some(ingress),
            Self::Trusted
            | Self::Peer {
                payload: RemotePayloadOrigin::Trusted,
                ..
            } => None,
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
    fn canonicalize(mut keys: Vec<DependencyKey>, max: usize) -> Result<Self, DependencySetError> {
        keys.sort_unstable();
        keys.dedup();
        if keys.len() > max {
            return Err(DependencySetError::TooMany);
        }
        Ok(Self(keys.into()))
    }

    fn canonicalize_nonempty(
        keys: Vec<DependencyKey>,
        max: usize,
    ) -> Result<Self, DependencySetError> {
        let dependencies = Self::canonicalize(keys, max)?;
        if dependencies.0.is_empty() {
            return Err(DependencySetError::Empty);
        }
        Ok(dependencies)
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
        Self::canonicalize(keys, capacity)
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
        Self::canonicalize(keys, max)
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
        Self::canonicalize(keys, max)
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
        KnownDependencies::canonicalize_nonempty(keys, max).map(Self)
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
    /// The complete resolve result is the physical payload consumed by script
    /// verification and, after dep compaction, by block-template DAO
    /// calculation. Keeping it in the same sealed value as the logical
    /// footprint prevents a later template path from re-resolving or pairing
    /// metadata with a different transaction.
    resolved: Arc<ResolvedTransaction>,
    identity: TxIdentity,
    pub(super) footprint: ExpandedFootprint,
    dependencies: KnownDependencies,
    fee: Capacity,
    serialized_bytes: usize,
    resolved_resident_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvidenceError {
    Footprint(FootprintError),
    DependencySet(DependencySetError),
    NotAnInput,
    NotADependency,
    ResidentBelowSerialized,
}

impl ResolvedPayload {
    /// Constructs immutable content facts from the exact transaction.
    /// Tip-bound input roles are sealed separately in `CellLocationReceipt`.
    pub(super) fn from_resolution(
        _seal: super::work::ResolutionSeal,
        resolved: Arc<ResolvedTransaction>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
    ) -> Result<Self, InputEvidenceError> {
        Self::from_resolved_parts(resolved, max_edges, fee, resolved_resident_bytes)
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        tx: &TransactionView,
        mut expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
        chain_inputs: Vec<OutPoint>,
        chain_dependencies: Vec<OutPoint>,
    ) -> Result<FoundationResolution, InputEvidenceError> {
        let mut chain_inputs = chain_inputs;
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        let mut chain_dependencies = chain_dependencies;
        chain_dependencies.sort_unstable();
        chain_dependencies.dedup();
        let mut resolved = ResolvedTransaction::dummy_resolve(tx.clone());
        if chain_inputs.iter().any(|input| {
            resolved
                .resolved_inputs
                .iter()
                .all(|cell| &cell.out_point != input)
        }) {
            return Err(InputEvidenceError::NotAnInput);
        }
        for cell in &mut resolved.resolved_inputs {
            if chain_inputs.binary_search(&cell.out_point).is_ok() {
                cell.transaction_info = Some(ckb_types::core::TransactionInfo::new(
                    1,
                    ckb_types::core::EpochNumberWithFraction::new(1, 0, 1),
                    Byte32::zero(),
                    1,
                ));
            }
        }
        expanded_dependencies.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| dependency.out_point()),
        );
        expanded_dependencies.sort_unstable();
        expanded_dependencies.dedup();
        resolved.resolved_cell_deps = expanded_dependencies
            .into_iter()
            .map(|out_point| {
                let mut cell = ckb_types::core::cell::CellMetaBuilder::default()
                    .out_point(out_point)
                    .build();
                if chain_dependencies.binary_search(&cell.out_point).is_ok() {
                    cell.transaction_info = Some(ckb_types::core::TransactionInfo::new(
                        1,
                        ckb_types::core::EpochNumberWithFraction::new(1, 0, 1),
                        Byte32::zero(),
                        1,
                    ));
                }
                cell
            })
            .collect();
        resolved.resolved_dep_groups.clear();
        if chain_dependencies.iter().any(|dependency| {
            resolved
                .related_dep_out_points()
                .all(|resolved| resolved != dependency)
        }) {
            return Err(InputEvidenceError::NotADependency);
        }
        let payload =
            Self::from_resolved_parts(Arc::new(resolved), max_edges, fee, resolved_resident_bytes)?;
        let location = CellLocationReceipt::from_resolution(&ChainViewId::initial(), &payload);
        Ok(FoundationResolution { payload, location })
    }

    fn from_resolved_parts(
        resolved: Arc<ResolvedTransaction>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
    ) -> Result<Self, InputEvidenceError> {
        let tx = &resolved.transaction;
        let dependency_capacity = resolved
            .resolved_cell_deps
            .len()
            .checked_add(resolved.resolved_dep_groups.len())
            .ok_or(InputEvidenceError::Footprint(FootprintError::Arithmetic))?;
        let mut expanded_dependencies = Vec::new();
        expanded_dependencies
            .try_reserve(dependency_capacity)
            .map_err(|_| InputEvidenceError::DependencySet(DependencySetError::Allocation))?;
        expanded_dependencies.extend(resolved.related_dep_out_points().cloned());
        let footprint = ExpandedFootprint::from_transaction(tx, expanded_dependencies, max_edges)
            .map_err(InputEvidenceError::Footprint)?;
        let dependencies = KnownDependencies::from_footprint(&footprint, max_edges)
            .map_err(InputEvidenceError::DependencySet)?;
        let serialized_bytes = tx.data().total_size();
        if resolved_resident_bytes < serialized_bytes {
            return Err(InputEvidenceError::ResidentBelowSerialized);
        }
        let identity = TxIdentity::from_transaction(tx);
        Ok(Self {
            resolved,
            identity,
            footprint,
            dependencies,
            fee,
            serialized_bytes,
            resolved_resident_bytes,
        })
    }

    pub(super) fn identity(&self) -> &TxIdentity {
        &self.identity
    }

    pub(super) fn resolved_transaction(&self) -> &Arc<ResolvedTransaction> {
        &self.resolved
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

/// Test-only complete resolution fixture. Production builds create the same
/// pair only by consuming checked-out ResolveWork.
#[cfg(test)]
pub(super) struct FoundationResolution {
    payload: ResolvedPayload,
    location: CellLocationReceipt,
}

#[cfg(test)]
impl FoundationResolution {
    pub(super) fn into_parts(self) -> (ResolvedPayload, CellLocationReceipt) {
        (self.payload, self.location)
    }

    pub(super) fn into_payload(self) -> ResolvedPayload {
        self.payload
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.location.is_chain_input(input)
    }

    pub(super) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.location.is_chain_dependency(dependency)
    }
}

#[cfg(test)]
impl std::ops::Deref for FoundationResolution {
    type Target = ResolvedPayload;

    fn deref(&self) -> &Self::Target {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedFacts {
    chain_view: ChainViewId,
    dependency_cut: DependencyCut,
    content: CellContentReceipt,
    location: CellLocationReceipt,
    verify_class: VerifyCycleClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VerifiedFacts {
    dependency_cut: DependencyCut,
    content: CellContentReceipt,
    context: VerificationContextReceipt,
    script: ScriptReceipt,
    metrics: CandidateMetrics,
}

impl ResolvedFacts {
    pub(super) fn from_resolution(
        _seal: super::work::ResolutionSeal,
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        location: CellLocationReceipt,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self {
            chain_view,
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            location,
            verify_class,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        chain_revision: ChainRevision,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self::for_foundation_view(
            ChainViewId::new(chain_revision, Byte32::zero()),
            dependency_cut,
            payload,
            verify_class,
        )
    }

    #[cfg(test)]
    pub(super) fn for_foundation_view(
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        let location = CellLocationReceipt::empty_for_foundation(&chain_view);
        Self {
            chain_view,
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            location,
            verify_class,
        }
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.content.payload()
    }

    pub(super) fn verify_class(&self) -> VerifyCycleClass {
        self.verify_class
    }

    pub(super) fn into_verification_parts(
        self,
        _seal: super::work::VerificationSeal,
    ) -> (
        DependencyCut,
        CellContentReceipt,
        CellLocationReceipt,
        VerifyCycleClass,
    ) {
        (
            self.dependency_cut,
            self.content,
            self.location,
            self.verify_class,
        )
    }
}

impl VerifiedFacts {
    pub(super) fn from_verification(
        _seal: super::work::VerificationSeal,
        dependency_cut: DependencyCut,
        content: CellContentReceipt,
        context: VerificationContextReceipt,
        metrics: CandidateMetrics,
    ) -> Self {
        let rules = context.rules();
        Self {
            dependency_cut,
            content,
            context,
            script: ScriptReceipt::from_verification(rules),
            metrics,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        chain_revision: ChainRevision,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        Self::for_foundation_view(
            ChainViewId::new(chain_revision, Byte32::zero()),
            dependency_cut,
            payload,
            metrics,
        )
    }

    #[cfg(test)]
    pub(super) fn for_foundation_view(
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        let rules = ValidationRulesId::FOUNDATION;
        let context = VerificationContextReceipt::empty_for_foundation(chain_view, rules);
        Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            context,
            script: ScriptReceipt::from_verification(rules),
            metrics,
        }
    }

    pub(super) fn witness(&self) -> &WitnessTxHash {
        &self.payload().identity().witness
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        self.context.view()
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.content.payload()
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.context.is_chain_input(input)
    }

    pub(super) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.context.is_chain_dependency(dependency)
    }

    pub(super) fn context_is_for(&self, view: &ChainViewId) -> bool {
        self.context.is_for(view)
    }

    pub(super) fn verification_context(&self) -> &VerificationContextReceipt {
        &self.context
    }

    pub(super) fn with_context(self, context: VerificationContextReceipt) -> Option<Self> {
        if !self.script.is_reusable_under(context.rules()) {
            return None;
        }
        Some(Self { context, ..self })
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
            Self::Any => match class {
                VerifyCycleClass::Small | VerifyCycleClass::Large => true,
            },
            Self::SmallCycleOnly => match class {
                VerifyCycleClass::Small => true,
                VerifyCycleClass::Large => false,
            },
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
    pub(super) chain_view: ChainViewId,
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
    Empty,
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
        let dependencies = KnownDependencies::canonicalize_nonempty(dependencies, max)
            .map_err(|_| DependencyObservationError::Empty)?;
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
    /// A missing-dependency observation owned by an ordinary admission. RBF
    /// replacement history is a distinct [`OwnedTx`] location, so the type
    /// cannot encode a schedulable history entry or a non-history conflict.
    Waiting(ObservedDependencies),
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
    pub(super) source: PreAcceptedSource,
    pub(super) basis: AdmissionBasis,
    pub(super) phase: PreAcceptedPhase,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug)]
pub(super) struct AcceptedEntry {
    pub(super) record: TxRecord,
    pub(super) provenance: AcceptedProvenance,
    pub(super) proof: AcceptedProof,
    pub(super) proposal: ProposalContextReceipt,
    pub(super) accepted_at: AcceptedAtMillis,
}

/// A previously Accepted member retained by one successful RBF Apply.
///
/// This is deliberately not a `PreAcceptedEntry`: it has no ingress source,
/// compute attribution, deadline, scheduler lane or executable phase. The
/// only legal transition out is a typed promotion/recovery or removal.
#[derive(Clone, Debug)]
pub(super) struct ReplacementHistoryEntry {
    record: TxRecord,
    /// Raw ingress/recovery basis retained for a full fresh resolve.
    basis: AdmissionBasis,
    /// `observed` is only the projected-final unavailable trigger set;
    /// `retained` is the complete expanded dependency proof. Keeping those
    /// roles separate prevents unrelated chain activity from prematurely
    /// consuming optional RBF recovery history.
    observed: ObservedDependencies,
    charge: ResourceVector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReplacementHistoryError {
    InvalidRecoveryTrigger,
    ResourceArithmetic,
    ResourceAllocation,
}

#[derive(Clone, Debug)]
pub(super) enum OwnedTx {
    PreAccepted(PreAcceptedEntry),
    Accepted(AcceptedEntry),
    ReplacementHistory(ReplacementHistoryEntry),
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
            PreAcceptedPhase::Waiting(observed) => observed.retained(),
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
            residency_peer: self.source.ingress_peer(),
            compute_peer,
        }
    }
}

impl ReplacementHistoryEntry {
    pub(super) fn from_accepted(
        accepted: &AcceptedEntry,
        recovery_triggers: MissingDependencies,
        version: EntryVersion,
        arrival: Arrival,
        dependency_cut: DependencyCut,
    ) -> Result<Self, ReplacementHistoryError> {
        let tx = &accepted.record.tx;
        let raw_edges = tx
            .inputs()
            .len()
            .checked_add(tx.cell_deps().len())
            .and_then(|count| count.checked_add(tx.header_deps().len()))
            .ok_or(ReplacementHistoryError::ResourceArithmetic)?;
        let declared_dependencies =
            KnownDependencies::from_transaction(tx).map_err(|error| match error {
                DependencySetError::Allocation => ReplacementHistoryError::ResourceAllocation,
                DependencySetError::Empty
                | DependencySetError::TooMany
                | DependencySetError::Arithmetic => ReplacementHistoryError::ResourceArithmetic,
            })?;
        let dependencies = accepted.proof.payload().dependencies().clone();
        if recovery_triggers
            .keys()
            .iter()
            .any(|key| !dependencies.contains(key))
        {
            return Err(ReplacementHistoryError::InvalidRecoveryTrigger);
        }
        let observed = ObservedDependencies::from_missing(
            &recovery_triggers,
            dependencies.clone(),
            dependency_cut,
        );
        let bytes = tx.data().total_size();
        let recovery_charge = ResourceVector::new(1, bytes, raw_edges, 0);
        let mut record = accepted.record.clone();
        record.version = version;
        record.arrival = arrival;
        Ok(Self {
            record,
            basis: AdmissionBasis::new(declared_dependencies, recovery_charge),
            observed,
            // History is a continuous reservation for its later Recovery
            // owner. CKB permits one outpoint to occur in different roles
            // (for example input + cell-dep), while the dependency frontier
            // canonicalizes those roles into one key. Retain the larger of
            // the encoded and canonical edge costs so wakeup never requires
            // an unplanned resource increase.
            charge: ResourceVector::new(1, bytes, raw_edges.max(dependencies.len()), 0),
        })
    }

    pub(super) fn record(&self) -> &TxRecord {
        &self.record
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        self.observed.retained()
    }

    pub(super) fn observation(&self) -> &ObservedDependencies {
        &self.observed
    }

    pub(super) fn charge(&self) -> ResourceVector {
        self.charge
    }

    pub(super) fn recovery_charge(&self) -> ResourceVector {
        self.basis.charge()
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord::ReplacementHistory(self.charge)
    }

    pub(super) fn into_recovery(
        self,
        generation: PoolGeneration,
        version: EntryVersion,
    ) -> PreAcceptedEntry {
        let charge = self.recovery_charge();
        let mut record = self.record;
        record.version = version;
        PreAcceptedEntry {
            record,
            source: PreAcceptedSource::Recovery(RecoveryLease { generation }),
            basis: self.basis,
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge,
        }
    }
}

impl AcceptedEntry {
    pub(super) fn status(&self) -> AcceptedStatus {
        self.proposal.status()
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        ChargeRecord::Accepted(AcceptedResources::one(self.proof.metrics().cost))
    }
}

impl OwnedTx {
    pub(super) fn record(&self) -> &TxRecord {
        match self {
            Self::PreAccepted(entry) => &entry.record,
            Self::Accepted(entry) => &entry.record,
            Self::ReplacementHistory(entry) => entry.record(),
        }
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        match self {
            Self::PreAccepted(entry) => entry.charge_record(),
            Self::Accepted(entry) => entry.charge_record(),
            Self::ReplacementHistory(entry) => entry.charge_record(),
        }
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        match self {
            Self::PreAccepted(entry) => entry.dependencies(),
            Self::Accepted(entry) => entry.proof.payload().dependencies(),
            Self::ReplacementHistory(entry) => entry.dependencies(),
        }
    }

    pub(super) fn ingress_peer(&self) -> Option<PeerIndex> {
        match self {
            Self::PreAccepted(entry) => entry.source.ingress_peer(),
            Self::Accepted(entry) => entry.provenance.ingress_peer(),
            Self::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn payload_blame_peer(&self) -> Option<PeerIndex> {
        match self {
            Self::PreAccepted(entry) => entry.source.payload_blame_peer(),
            Self::Accepted(entry) => entry.provenance.payload_blame_peer(),
            Self::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn preaccepted_charge(&self) -> Option<ResourceVector> {
        match self {
            Self::PreAccepted(entry) => Some(entry.charge),
            Self::Accepted(_) | Self::ReplacementHistory(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ValidatedAdmission {
    pub(super) tx: Arc<TransactionView>,
    pub(super) identity: TxIdentity,
    pub(super) source: PreAcceptedSource,
    pub(super) dependencies: KnownDependencies,
    pub(super) charge: ResourceVector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AdmissionValidationError {
    EmptyTransaction,
    ResourceArithmetic,
    ResourceAllocation,
}

impl ValidatedAdmission {
    pub(super) fn remote(
        tx: TransactionView,
        peer: PeerIndex,
    ) -> Result<Self, AdmissionValidationError> {
        Self::remote_with_lease(tx, RemoteResidencyLease::for_foundation(peer))
    }

    pub(super) fn remote_with_lease(
        tx: TransactionView,
        residency: RemoteResidencyLease,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            PreAcceptedSource::Remote(RemoteBase::ingress(residency)),
        )
    }

    pub(super) fn proposal(
        tx: TransactionView,
        context: ProposalContextId,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            PreAcceptedSource::Proposal {
                lease: ProposalLease { context },
                base: ProposalBase::Trusted,
            },
        )
    }

    pub(super) fn recovery(
        tx: TransactionView,
        generation: PoolGeneration,
    ) -> Result<Self, AdmissionValidationError> {
        Self::new(
            tx,
            PreAcceptedSource::Recovery(RecoveryLease { generation }),
        )
    }

    fn new(
        tx: TransactionView,
        source: PreAcceptedSource,
    ) -> Result<Self, AdmissionValidationError> {
        let bytes = tx.data().total_size();
        if bytes == 0 {
            return Err(AdmissionValidationError::EmptyTransaction);
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
            source,
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
