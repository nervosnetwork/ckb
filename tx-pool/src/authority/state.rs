use super::chain::{
    AcceptedProof, CellContentReceipt, CellLocationReceipt, CellLocationReceiptError,
    ProposalContextReceipt, ScriptReceipt, TimeContextReceipt, VerificationContextReceipt,
};
use super::resources::{
    AcceptedCost, AcceptedResources, ChargeRecord, ComputeGrant, ReplacementHistoryCharge,
    ResourceVector,
};
use crate::{
    component::entry::{accepted_transaction_charge_bytes, resolved_transaction_charge_bytes},
    util::compact_packed,
};
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::{sync::Arc, time::Instant};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RawTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct WitnessTxHash(pub(super) Byte32);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ProposalId(pub(super) ProposalShortId);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryVersion(pub(super) u128);

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

/// Monotonic start of one retained (asynchronous) verification attempt.
/// This observability capability exists only from verification preparation
/// through final admission; it is stripped before Accepted ownership so
/// metrics do not become long-lived transaction state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AsyncProcessStart(Instant);

impl AsyncProcessStart {
    pub(super) fn now() -> Self {
        Self(Instant::now())
    }

    pub(super) fn elapsed_seconds(self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveryLease {
    pub(super) generation: PoolGeneration,
}

/// Validation authority attached to the exact witness payload checked out by
/// a worker. A same-witness Proposal promotion can replace this policy while
/// the old capability is active, so settlement compares the sealed value with
/// the current owner instead of consulting mutable source state mid-verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PayloadPolicy {
    RemoteDeclaredCycles(super::ingress::RemoteCycleLimit),
    Trusted,
}

/// The complete payload-policy relation visible to one settlement. An active
/// peer claim may be superseded only by trusted evidence for the same owner
/// version; every other policy change is structural corruption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PayloadPolicyEvolution {
    Unchanged,
    RemoteToTrusted,
    Invalid,
}

impl PayloadPolicy {
    pub(super) const fn evolution_to(self, current: Self) -> PayloadPolicyEvolution {
        match (self, current) {
            (Self::RemoteDeclaredCycles(active), Self::RemoteDeclaredCycles(current))
                if active.declared() == current.declared() =>
            {
                PayloadPolicyEvolution::Unchanged
            }
            (Self::Trusted, Self::Trusted) => PayloadPolicyEvolution::Unchanged,
            (Self::RemoteDeclaredCycles(_), Self::Trusted) => {
                PayloadPolicyEvolution::RemoteToTrusted
            }
            (Self::RemoteDeclaredCycles(_), Self::RemoteDeclaredCycles(_))
            | (Self::Trusted, Self::RemoteDeclaredCycles(_)) => PayloadPolicyEvolution::Invalid,
        }
    }

    /// Exact resolution-time verifier lane. Trusted work never inherits a
    /// peer-declared cost, while Remote work enters the large lane iff its
    /// declaration is strictly above the configured small-worker threshold.
    pub(super) const fn verify_cycle_class(
        self,
        large_cycle_threshold: ckb_types::core::Cycle,
    ) -> VerifyCycleClass {
        match self {
            Self::RemoteDeclaredCycles(limit) if limit.declared() > large_cycle_threshold => {
                VerifyCycleClass::Large
            }
            Self::RemoteDeclaredCycles(_) | Self::Trusted => VerifyCycleClass::Small,
        }
    }

    pub(super) const fn declared_cycles(self) -> Option<ckb_types::core::Cycle> {
        match self {
            Self::RemoteDeclaredCycles(limit) => Some(limit.declared()),
            Self::Trusted => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteBase {
    pub(super) residency: RemoteResidencyLease,
    pub(super) payload_policy: PayloadPolicy,
}

impl RemoteBase {
    pub(super) const fn ingress(
        residency: RemoteResidencyLease,
        declared_limit: super::ingress::RemoteCycleLimit,
    ) -> Self {
        Self {
            residency,
            payload_policy: PayloadPolicy::RemoteDeclaredCycles(declared_limit),
        }
    }

    pub(super) const fn blame_peer(self) -> Option<PeerIndex> {
        match self.payload_policy {
            PayloadPolicy::RemoteDeclaredCycles(_) => Some(self.residency.peer),
            PayloadPolicy::Trusted => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProposalBase {
    Trusted,
    /// A trusted proposal promoted an earlier Remote owner. Only immutable
    /// ingress residency survives: proposal evidence permanently supersedes
    /// the peer-declared payload policy, so that policy is unrepresentable in
    /// this state.
    Remote(RemoteResidencyLease),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreAcceptedSource {
    Remote(RemoteBase),
    Proposal { base: ProposalBase },
    Recovery(RecoveryLease),
}

impl PreAcceptedSource {
    pub(super) const fn ingress_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(remote) => Some(remote.residency.peer),
            Self::Proposal {
                base: ProposalBase::Remote(residency),
                ..
            } => Some(residency.peer),
            Self::Proposal {
                base: ProposalBase::Trusted,
                ..
            }
            | Self::Recovery(_) => None,
        }
    }

    pub(super) const fn payload_blame_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(remote) => remote.blame_peer(),
            Self::Proposal { .. } | Self::Recovery(_) => None,
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

    pub(super) const fn payload_policy(self) -> PayloadPolicy {
        match self {
            Self::Remote(remote) => remote.payload_policy,
            Self::Proposal { .. } | Self::Recovery(_) => PayloadPolicy::Trusted,
        }
    }

    pub(super) const fn accepted_provenance(self) -> AcceptedProvenance {
        match self {
            Self::Remote(remote) => AcceptedProvenance::Peer {
                ingress: remote.residency.peer,
            },
            Self::Proposal {
                base: ProposalBase::Remote(residency),
                ..
            } => AcceptedProvenance::Peer {
                ingress: residency.peer,
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
    Peer { ingress: PeerIndex },
}

impl AcceptedProvenance {
    pub(super) const fn ingress_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Trusted => None,
            Self::Peer { ingress } => Some(ingress),
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
pub(super) struct KnownDependencies(Arc<Vec<DependencyKey>>);

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
        Ok(Self(Arc::new(keys)))
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

    fn from_bounded_transaction(
        tx: &TransactionView,
        encoded_edges: usize,
    ) -> Result<Self, std::collections::TryReserveError> {
        let mut keys = Vec::new();
        keys.try_reserve_exact(encoded_edges)?;
        keys.extend(tx.input_pts_iter().map(DependencyKey::Cell));
        keys.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| DependencyKey::Cell(dependency.out_point())),
        );
        keys.extend(tx.header_deps().into_iter().map(DependencyKey::Header));
        keys.sort_unstable();
        keys.dedup();
        Ok(Self(Arc::new(keys)))
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
        self.0.as_slice()
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

    pub(super) fn parent_transactions(&self) -> Result<Arc<Vec<RawTxHash>>, DependencySetError> {
        parent_transactions(&self.0)
    }
}

fn parent_transactions(
    dependencies: &KnownDependencies,
) -> Result<Arc<Vec<RawTxHash>>, DependencySetError> {
    let mut parents = Vec::new();
    parents
        .try_reserve(dependencies.len())
        .map_err(|_| DependencySetError::Allocation)?;
    for key in dependencies.keys() {
        let DependencyKey::Cell(out_point) = key else {
            continue;
        };
        let parent = RawTxHash(compact_packed(&out_point.tx_hash()));
        if parents.last() != Some(&parent) {
            parents.push(parent);
        }
    }
    Ok(Arc::new(parents))
}

#[derive(Debug, PartialEq, Eq)]
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
    Allocation,
    Arithmetic,
}

impl ExpandedFootprint {
    pub(super) fn from_transaction(
        tx: &TransactionView,
        mut expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
    ) -> Result<Self, FootprintError> {
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(tx.inputs().len())
            .map_err(|_| FootprintError::Allocation)?;
        inputs.extend(tx.input_pts_iter());
        let input_count = inputs.len();
        inputs.sort_unstable();
        inputs.dedup();
        if inputs.len() != input_count {
            return Err(FootprintError::DuplicateInput);
        }

        expanded_dependencies
            .try_reserve(tx.cell_deps().len())
            .map_err(|_| FootprintError::Allocation)?;
        expanded_dependencies.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| dependency.out_point()),
        );
        expanded_dependencies.sort_unstable();
        expanded_dependencies.dedup();
        expanded_dependencies.retain(|dependency| inputs.binary_search(dependency).is_err());
        let headers = tx.header_deps();
        let mut header_dependencies = Vec::new();
        header_dependencies
            .try_reserve_exact(headers.len())
            .map_err(|_| FootprintError::Allocation)?;
        header_dependencies.extend(headers);
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
    pub(super) footprint: Arc<ExpandedFootprint>,
    dependencies: KnownDependencies,
    fee: Capacity,
    serialized_bytes: usize,
    resolved_resident_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvidenceError {
    Footprint(FootprintError),
    DependencySet(DependencySetError),
    ResidentBelowSerialized,
}

/// Public disposition class for failures while sealing resolved cell facts.
///
/// Both retained and direct admission consume this one exhaustive map. This
/// prevents a hostile transaction shape from becoming a worker fault on one
/// path while being rejected normally on the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvidenceDisposition {
    MalformedTransaction,
    ResourceDenied,
    ResourceUnavailable,
    Structural,
}

impl InputEvidenceError {
    pub(super) const fn disposition(&self) -> InputEvidenceDisposition {
        match self {
            Self::Footprint(FootprintError::DuplicateInput) => {
                InputEvidenceDisposition::MalformedTransaction
            }
            Self::Footprint(FootprintError::TooManyEdges)
            | Self::DependencySet(DependencySetError::TooMany) => {
                InputEvidenceDisposition::ResourceDenied
            }
            Self::Footprint(FootprintError::Allocation)
            | Self::DependencySet(DependencySetError::Allocation) => {
                InputEvidenceDisposition::ResourceUnavailable
            }
            Self::Footprint(FootprintError::Arithmetic)
            | Self::DependencySet(DependencySetError::Empty | DependencySetError::Arithmetic)
            | Self::ResidentBelowSerialized => InputEvidenceDisposition::Structural,
        }
    }
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

    pub(super) fn from_direct_resolution(
        _seal: super::resolver::DirectResolutionSeal,
        resolved: Arc<ResolvedTransaction>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
    ) -> Result<Self, InputEvidenceError> {
        Self::from_resolved_parts(resolved, max_edges, fee, resolved_resident_bytes)
    }

    /// Build the synthetic resolved payload accepted only by the sealed
    /// `internal` instrumentation boundary. `serialized_bytes` deliberately
    /// preserves the caller-provided `TxEntry` weight used by historical
    /// package-selection tests; no normal admission path can construct the
    /// seal or substitute this value for the transaction's encoded size.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn from_internal_plug(
        _seal: super::internal::InternalPlugSeal,
        resolved: Arc<ResolvedTransaction>,
        max_edges: usize,
        fee: Capacity,
        serialized_bytes: usize,
        resolved_resident_bytes: usize,
    ) -> Result<Self, InputEvidenceError> {
        let mut payload =
            Self::from_resolved_parts(resolved, max_edges, fee, resolved_resident_bytes)?;
        payload.serialized_bytes = serialized_bytes;
        Ok(payload)
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
        let footprint = Arc::new(
            ExpandedFootprint::from_transaction(tx, expanded_dependencies, max_edges)
                .map_err(InputEvidenceError::Footprint)?,
        );
        let dependencies = KnownDependencies::from_footprint(&footprint, max_edges)
            .map_err(InputEvidenceError::DependencySet)?;
        let payload_bytes = tx.data().total_size();
        if resolved_resident_bytes < payload_bytes {
            return Err(InputEvidenceError::ResidentBelowSerialized);
        }
        // Accepted-pool size, fee-rate ordering and RBF policy use the exact
        // bytes occupied by a transaction in a block. This includes the
        // Molecule vector offset and intentionally differs from the raw
        // payload allocation charged while the transaction is PreAccepted.
        let serialized_bytes = tx.data().serialized_size_in_block();
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

    pub(super) fn footprint(&self) -> &Arc<ExpandedFootprint> {
        &self.footprint
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

    /// Consume full script-verification evidence at the only successful
    /// Verify boundary and retain only the payload required by final tx-pool
    /// validation, Accepted membership and block-template DAO accounting.
    ///
    /// Resolved inputs remain complete. Cell deps keep their outpoint and
    /// transaction information, which is sufficient for liveness and
    /// maturity; their scripts/data cannot be reused for a later VM run. A
    /// rules change must therefore return the owner to Resolve, not Verify.
    pub(super) fn compact_after_verification(
        payload: Arc<Self>,
        _seal: super::work::VerificationSeal,
    ) -> (Arc<Self>, usize) {
        Self::compact_verified(payload)
    }

    pub(super) fn compact_after_direct_verification(
        payload: Arc<Self>,
        _seal: super::resolver::DirectVerificationSeal,
    ) -> (Arc<Self>, usize) {
        Self::compact_verified(payload)
    }

    fn compact_verified(payload: Arc<Self>) -> (Arc<Self>, usize) {
        let mut payload = match Arc::try_unwrap(payload) {
            Ok(payload) => payload,
            Err(shared) => {
                let accepted_resident_bytes = accepted_transaction_charge_bytes(
                    shared.serialized_bytes,
                    shared.resolved_transaction(),
                );
                return (shared, accepted_resident_bytes);
            }
        };
        payload.resolved = super::residency::compact_after_verification(payload.resolved);
        payload.resolved_resident_bytes = resolved_transaction_charge_bytes(
            payload.serialized_bytes,
            payload.resolved_transaction(),
        );
        let accepted_resident_bytes = accepted_transaction_charge_bytes(
            payload.serialized_bytes,
            payload.resolved_transaction(),
        );
        (Arc::new(payload), accepted_resident_bytes)
    }

    /// Replace only tip-relative cell locations after lock-external
    /// validation. The unforgeable seal is owned by the production validator,
    /// so callers cannot use this operation to substitute transaction content
    /// while retaining an earlier script receipt.
    pub(super) fn with_refreshed_locations(
        &self,
        _seal: super::validation::LocationRefreshSeal,
        resolved: Arc<ResolvedTransaction>,
        fee: Capacity,
    ) -> Self {
        let resolved_resident_bytes =
            resolved_transaction_charge_bytes(self.serialized_bytes, &resolved);
        Self {
            resolved,
            identity: self.identity.clone(),
            footprint: Arc::clone(&self.footprint),
            dependencies: self.dependencies.clone(),
            fee,
            serialized_bytes: self.serialized_bytes,
            resolved_resident_bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedFacts {
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
    verify_class: VerifyCycleClass,
    metrics: CandidateMetrics,
    async_process_start: Option<AsyncProcessStart>,
}

impl ResolvedFacts {
    pub(super) fn from_resolution(
        _seal: super::work::ResolutionSeal,
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Result<Self, CellLocationReceiptError> {
        let location = CellLocationReceipt::from_resolution(chain_view, &payload)?;
        Ok(Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            location,
            verify_class,
        })
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        self.location.view()
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
        seal: super::work::VerificationSeal,
        time: TimeContextReceipt,
    ) -> (
        DependencyCut,
        CellContentReceipt,
        VerificationContextReceipt,
        VerifyCycleClass,
    ) {
        let context = VerificationContextReceipt::from_resolved(seal, self.location, time);
        (
            self.dependency_cut,
            self.content,
            context,
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
        verify_class: VerifyCycleClass,
        metrics: CandidateMetrics,
        async_process_start: AsyncProcessStart,
    ) -> Self {
        let rules = context.rules();
        Self {
            dependency_cut,
            content,
            context,
            script: ScriptReceipt::from_verification(rules),
            verify_class,
            metrics,
            async_process_start: Some(async_process_start),
        }
    }

    pub(super) fn from_direct_verification(
        _seal: super::resolver::DirectVerificationSeal,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        context: VerificationContextReceipt,
        metrics: CandidateMetrics,
    ) -> Self {
        let rules = context.rules();
        Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            context,
            script: ScriptReceipt::from_verification(rules),
            verify_class: VerifyCycleClass::Small,
            metrics,
            async_process_start: None,
        }
    }

    /// Seal synthetic script evidence for the feature-internal `PlugEntry`
    /// adapter. This exists solely to preserve the established test hook that
    /// injects an already-verified `TxEntry`; it is not reachable from RPC,
    /// relay, persistence replay, or ordinary Local admission.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn from_internal_plug(
        _seal: super::internal::InternalPlugSeal,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        context: VerificationContextReceipt,
        metrics: CandidateMetrics,
    ) -> Self {
        let rules = context.rules();
        Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            context,
            script: ScriptReceipt::from_verification(rules),
            verify_class: VerifyCycleClass::Small,
            metrics,
            async_process_start: None,
        }
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

    pub(super) fn payload_arc(&self) -> &Arc<ResolvedPayload> {
        self.content.payload_arc()
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }

    pub(super) fn into_accepted(mut self) -> (Self, Option<AsyncProcessStart>) {
        let async_process_start = self.async_process_start.take();
        (self, async_process_start)
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

    /// Rebind dependency observation only after the final validator has
    /// rechecked every resolved cell against the authority cut that issued
    /// `dependency_cut`. Script and cell-content evidence remain unchanged.
    pub(super) fn with_validated_dependency_cut(
        self,
        _seal: super::validation::AdmissionValidationSeal,
        dependency_cut: DependencyCut,
    ) -> Self {
        Self {
            dependency_cut,
            ..self
        }
    }

    /// Commit the final validator's authoritative cell-location cut to both
    /// consumers in one transition. Block construction reads the payload's
    /// `TransactionInfo`, while policy reads the context provenance; neither
    /// projection can be updated independently by a production caller.
    pub(super) fn with_final_validation(
        self,
        _seal: super::validation::LocationRefreshSeal,
        payload: Arc<ResolvedPayload>,
        context: VerificationContextReceipt,
    ) -> Option<Self> {
        if !self.script.is_reusable_under(context.rules()) {
            return None;
        }
        let metrics = CandidateMetrics {
            fee: payload.fee(),
            cost: AcceptedCost::new(
                payload.serialized_bytes(),
                accepted_transaction_charge_bytes(
                    payload.serialized_bytes(),
                    payload.resolved_transaction(),
                ),
                self.metrics.cost.cycles,
            ),
        };
        Some(Self {
            content: CellContentReceipt::from_resolution(payload),
            context,
            metrics,
            ..self
        })
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QueuedWork {
    Resolve,
    Verify(ResolvedFacts),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActiveWork {
    pub(super) chain_view: ChainViewId,
    pub(super) permit: WorkPermit,
    pub(super) grant: ComputeGrant,
    pub(super) attribution: ComputeAttribution,
    pub(super) payload_policy: PayloadPolicy,
    pub(super) dependency_cut: DependencyCut,
    pub(super) dependencies: KnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ObservedDependencies {
    dependency_cut: DependencyCut,
    observed: KnownDependencies,
    retained: KnownDependencies,
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

    pub(super) fn parent_transactions(&self) -> Result<Arc<Vec<RawTxHash>>, DependencySetError> {
        parent_transactions(&self.observed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PreAcceptedPhase {
    Queued(QueuedWork),
    Computing(ActiveWork),
    /// A missing-dependency observation owned by an ordinary admission. RBF
    /// replacement history is a distinct [`OwnedTx`] location, so the type
    /// cannot encode a schedulable history entry or a non-history conflict.
    Waiting(ObservedDependencies),
    /// Final validation evidence waiting for the one membership disposition.
    /// Reject/cancel/budget outcomes are terminal or retry transitions and
    /// therefore cannot be represented as resident owner phases.
    Ready(VerifiedFacts),
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
    payload_bytes: usize,
    encoded_edges: usize,
    original_charge: ResourceVector,
}

impl AdmissionBasis {
    pub(super) fn new(
        declared_dependencies: KnownDependencies,
        payload_bytes: usize,
        encoded_edges: usize,
        original_charge: ResourceVector,
    ) -> Self {
        Self {
            declared_dependencies,
            payload_bytes,
            encoded_edges,
            original_charge,
        }
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        &self.declared_dependencies
    }

    pub(super) fn charge(&self) -> ResourceVector {
        self.original_charge
    }

    pub(super) fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    pub(super) fn encoded_edges(&self) -> usize {
        self.encoded_edges
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
            PreAcceptedPhase::Queued(QueuedWork::Resolve) => self.basis.dependencies(),
            PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => {
                resolved.payload().dependencies()
            }
            PreAcceptedPhase::Computing(active) => &active.dependencies,
            PreAcceptedPhase::Waiting(observed) => observed.retained(),
            PreAcceptedPhase::Ready(verified) => verified.payload().dependencies(),
        }
    }

    pub(super) fn original_charge(&self) -> ResourceVector {
        self.basis.charge()
    }

    pub(super) fn charge_record(&self) -> ChargeRecord {
        let compute_peer = match &self.phase {
            PreAcceptedPhase::Computing(active) => active.attribution.peer(),
            PreAcceptedPhase::Queued(_)
            | PreAcceptedPhase::Waiting(_)
            | PreAcceptedPhase::Ready(_) => None,
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
        charge: ReplacementHistoryCharge,
        version: EntryVersion,
        arrival: Arrival,
        dependency_cut: DependencyCut,
    ) -> Result<Self, ReplacementHistoryError> {
        let tx = &accepted.record.tx;
        let (payload_bytes, encoded_edges, recovery_charge, retained_charge) = charge.into_parts();
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
        let observed =
            ObservedDependencies::from_missing(&recovery_triggers, dependencies, dependency_cut);
        let mut record = accepted.record.clone();
        record.version = version;
        record.arrival = arrival;
        Ok(Self {
            record,
            basis: AdmissionBasis::new(
                declared_dependencies,
                payload_bytes,
                encoded_edges,
                recovery_charge,
            ),
            observed,
            // History is a continuous reservation for its later Recovery
            // owner. CKB permits one outpoint to occur in different roles
            // (for example input + cell-dep), while the dependency frontier
            // canonicalizes those roles into one key. Retain the larger of
            // the encoded and canonical edge costs so wakeup never requires
            // an unplanned resource increase.
            charge: retained_charge,
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
}

#[derive(Clone, Debug)]
pub(super) struct ValidatedAdmission {
    pub(super) tx: Arc<TransactionView>,
    pub(super) identity: TxIdentity,
    pub(super) source: PreAcceptedSource,
    pub(super) dependencies: KnownDependencies,
    pub(super) payload_bytes: usize,
    pub(super) encoded_edges: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryAdmissionError {
    InvalidTransaction,
    ResourceUnavailable,
}

#[derive(Debug)]
pub(super) struct RetainedAdmissionAllocation {
    transaction: Arc<TransactionView>,
}

impl RetainedAdmissionAllocation {
    pub(super) fn into_transaction(self) -> Arc<TransactionView> {
        self.transaction
    }
}

impl ValidatedAdmission {
    pub(super) fn recovery(
        tx: TransactionView,
        generation: PoolGeneration,
    ) -> Result<Self, RecoveryAdmissionError> {
        let tx = super::ingress::BoundedTransaction::try_new(tx).map_err(|error| match error {
            super::ingress::BoundedTransactionError::Allocation => {
                RecoveryAdmissionError::ResourceUnavailable
            }
            super::ingress::BoundedTransactionError::TooLarge { .. } => {
                RecoveryAdmissionError::InvalidTransaction
            }
        })?;
        Self::new(
            tx,
            PreAcceptedSource::Recovery(RecoveryLease { generation }),
        )
        .map_err(|_| RecoveryAdmissionError::ResourceUnavailable)
    }

    pub(super) fn from_retained_ingress(
        _seal: super::ingress::RetainedIngressSeal,
        tx: super::ingress::BoundedTransaction,
        source: PreAcceptedSource,
    ) -> Result<Self, RetainedAdmissionAllocation> {
        Self::new(tx, source)
    }

    fn new(
        tx: super::ingress::BoundedTransaction,
        source: PreAcceptedSource,
    ) -> Result<Self, RetainedAdmissionAllocation> {
        let (tx, payload_bytes, encoded_edges) = tx.into_admission_parts();
        let dependencies = match KnownDependencies::from_bounded_transaction(&tx, encoded_edges) {
            Ok(dependencies) => dependencies,
            Err(_) => {
                return Err(RetainedAdmissionAllocation { transaction: tx });
            }
        };
        // The reverse projection is a canonical set, but ingress accounting
        // deliberately charges every encoded edge. Duplicate declarations do
        // not buy an attacker extra pre-pool residency for free.
        Ok(Self {
            identity: TxIdentity::from_transaction(&tx),
            tx,
            source,
            dependencies,
            payload_bytes,
            encoded_edges,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuthorityClocks {
    pub(super) next_version: EntryVersion,
    pub(super) next_arrival: Arrival,
    pub(super) next_sequence: ApplySequence,
}

impl AuthorityClocks {
    pub(super) const fn first() -> Self {
        Self {
            next_version: EntryVersion(1),
            next_arrival: Arrival(0),
            next_sequence: ApplySequence(1),
        }
    }
}

#[cfg(test)]
#[path = "tests/support/state.rs"]
pub(in crate::authority) mod test_support;
