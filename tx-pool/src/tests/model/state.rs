use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) const EFFECT_ENVELOPE_BYTES: u32 = 4;
const EFFECT_ID_BYTES: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TxId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct WitnessId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ProposalId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CellId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct HeaderId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PeerId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ViewId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RulesId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct EntryVersion(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ApplyStamp(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Arrival(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CapabilityId(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct DirectRequestId(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PoolGeneration(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ChainRevision(pub(super) u16);

/// One exact installed chain occurrence. `tip` may repeat across an ABA
/// sequence; `revision` is the authority-local ordering token that makes the
/// occurrence unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ChainView {
    pub(super) tip: ViewId,
    pub(super) revision: ChainRevision,
}

impl ChainView {
    pub(super) const fn initial(tip: ViewId) -> Self {
        Self {
            tip,
            revision: ChainRevision(0),
        }
    }

    pub(super) fn advance(self, tip: ViewId) -> Option<Self> {
        Some(Self {
            tip,
            revision: ChainRevision(self.revision.0.checked_add(1)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct MonotonicTick(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RemoteDeadline(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RemoteResidency {
    pub(super) peer: PeerId,
    pub(super) expires_at: RemoteDeadline,
}

impl RemoteResidency {
    pub(super) const fn new(peer: PeerId, expires_at: RemoteDeadline) -> Self {
        Self { peer, expires_at }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct ResourceVector {
    pub(super) entries: u16,
    pub(super) bytes: u32,
    pub(super) edges: u16,
}

impl ResourceVector {
    pub(super) const ZERO: Self = Self {
        entries: 0,
        bytes: 0,
        edges: 0,
    };

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries && self.bytes <= limit.bytes && self.edges <= limit.edges
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ModelLimits {
    pub(super) owners: ResourceVector,
    pub(super) retained: ResourceVector,
    pub(super) accepted: ResourceVector,
    pub(super) replacement_history: ResourceVector,
    pub(super) remote_per_peer: ResourceVector,
    pub(super) effect_records: u16,
    pub(super) effect_bytes: u32,
    pub(super) compute_permits: u16,
    pub(super) peer_ban_fences: u16,
    pub(super) peer_ban_duration: u64,
}

impl ModelLimits {
    pub(super) const fn small() -> Self {
        Self {
            owners: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            retained: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            accepted: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            replacement_history: ResourceVector {
                entries: 2,
                bytes: 32,
                edges: 8,
            },
            remote_per_peer: ResourceVector {
                entries: 2,
                bytes: 32,
                edges: 8,
            },
            effect_records: 8,
            effect_bytes: 192,
            compute_permits: 2,
            peer_ban_fences: 2,
            peer_ban_duration: 10,
        }
    }

    pub(super) fn largest_indivisible_effect_batch(self) -> Option<(u16, u32)> {
        let records = self.owners.entries.checked_add(1)?;
        // One owner-sized direct candidate may be published together with one
        // effect for every resident owner. Resident effects may carry the
        // complete payload partition, while newly waiting Remote owners carry
        // the complete dependency-edge partition. Envelope bytes are charged
        // once per record. Adding all three independent terms is the bound;
        // taking their maximum would not cover a mixed RBF/dependency Apply.
        let bytes = self
            .owners
            .bytes
            .checked_mul(2)?
            .checked_add(u32::from(self.owners.edges).checked_mul(EFFECT_ID_BYTES)?)?
            .checked_add(u32::from(records).checked_mul(EFFECT_ENVELOPE_BYTES)?)?;
        Some((records, bytes))
    }

    pub(super) fn validate(self) -> Result<ValidatedLimits, ConfigurationError> {
        if self.compute_permits == 0
            || self.owners.entries == 0
            || self.effect_records == 0
            || self.effect_bytes == 0
            || self.peer_ban_fences == 0
            || self.peer_ban_duration == 0
        {
            return Err(ConfigurationError::ZeroCapacity);
        }
        let Some((largest_effect_batch_records, largest_effect_batch_bytes)) =
            self.largest_indivisible_effect_batch()
        else {
            return Err(ConfigurationError::IndivisibleEffectBatch);
        };
        if largest_effect_batch_records > self.effect_records
            || largest_effect_batch_bytes > self.effect_bytes
        {
            return Err(ConfigurationError::IndivisibleEffectBatch);
        }
        if !self.retained.fits(self.owners)
            || !self.accepted.fits(self.owners)
            || !self.replacement_history.fits(self.owners)
            || !self.replacement_history.fits(self.retained)
            || !self.remote_per_peer.fits(self.retained)
        {
            return Err(ConfigurationError::InvalidPartition);
        }
        Ok(ValidatedLimits(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ValidatedLimits(ModelLimits);

impl ValidatedLimits {
    pub(super) const fn get(self) -> ModelLimits {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ConfigurationError {
    ZeroCapacity,
    IndivisibleEffectBatch,
    InvalidPartition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum InputOrigin {
    Chain,
    Pool(TxId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PeerBanDeadline {
    At(MonotonicTick),
    ProcessLifetime,
}

impl PeerBanDeadline {
    pub(super) fn after(observed_at: MonotonicTick, duration: u64) -> Self {
        observed_at
            .0
            .checked_add(duration)
            .map_or(Self::ProcessLifetime, |deadline| {
                Self::At(MonotonicTick(deadline))
            })
    }

    pub(super) fn is_active_at(self, observed_at: MonotonicTick) -> bool {
        match self {
            Self::At(deadline) => deadline > observed_at,
            Self::ProcessLifetime => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PeerBanRecord {
    pub(super) deadline: PeerBanDeadline,
    pub(super) order: ApplyStamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Transaction {
    pub(super) id: TxId,
    pub(super) witness: WitnessId,
    pub(super) proposal: ProposalId,
    pub(super) inputs: BTreeSet<CellId>,
    pub(super) deps: BTreeSet<CellId>,
    pub(super) header_deps: BTreeSet<HeaderId>,
    pub(super) outputs: BTreeSet<CellId>,
    pub(super) bytes: u32,
    pub(super) fee: u64,
}

impl Transaction {
    pub(super) fn independent(id: u8, witness: u8, input: u8, output: u8) -> Self {
        Self {
            id: TxId(id),
            witness: WitnessId(witness),
            proposal: ProposalId(id),
            inputs: BTreeSet::from([CellId(input)]),
            deps: BTreeSet::new(),
            header_deps: BTreeSet::new(),
            outputs: BTreeSet::from([CellId(output)]),
            bytes: 4,
            fee: 10,
        }
    }

    pub(super) fn dependent(id: u8, witness: u8, input: u8, output: u8) -> Self {
        Self {
            id: TxId(id),
            witness: WitnessId(witness),
            proposal: ProposalId(id),
            inputs: BTreeSet::from([CellId(input)]),
            deps: BTreeSet::new(),
            header_deps: BTreeSet::new(),
            outputs: BTreeSet::from([CellId(output)]),
            bytes: 4,
            fee: 10,
        }
    }

    pub(super) fn charge(&self) -> Option<ResourceVector> {
        let edges = self
            .inputs
            .len()
            .checked_add(self.deps.len())?
            .checked_add(self.header_deps.len())?;
        Some(ResourceVector {
            entries: 1,
            bytes: self.bytes,
            edges: u16::try_from(edges).ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Source {
    Recovery(PoolGeneration),
    Proposal { base: ProposalBase },
    Remote(RemoteResidency),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum ProposalBase {
    Trusted,
    Remote(RemoteResidency),
}

impl Source {
    pub(super) const fn priority(self) -> u8 {
        match self {
            Self::Recovery(_) => 0,
            Self::Proposal { .. } => 1,
            Self::Remote(_) => 2,
        }
    }

    pub(super) const fn ingress_peer(self) -> Option<PeerId> {
        match self {
            Self::Proposal {
                base: ProposalBase::Remote(residency),
            }
            | Self::Remote(residency) => Some(residency.peer),
            Self::Proposal {
                base: ProposalBase::Trusted,
            } => None,
            Self::Recovery(_) => None,
        }
    }

    pub(super) const fn active_remote_deadline(self) -> Option<RemoteDeadline> {
        match self {
            Self::Remote(residency) => Some(residency.expires_at),
            Self::Recovery(_) | Self::Proposal { .. } => None,
        }
    }

    pub(super) const fn accepted_provenance(self) -> AcceptedProvenance {
        match self.ingress_peer() {
            Some(peer) => AcceptedProvenance::Peer(peer),
            None => AcceptedProvenance::Trusted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AcceptedProvenance {
    Trusted,
    Peer(PeerId),
}

impl AcceptedProvenance {
    pub(super) const fn ingress_peer(self) -> Option<PeerId> {
        match self {
            Self::Trusted => None,
            Self::Peer(peer) => Some(peer),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum RetainedSource {
    Recovery(PoolGeneration),
    Proposal,
    Remote(RemoteResidency),
}

impl From<RetainedSource> for Source {
    fn from(source: RetainedSource) -> Self {
        match source {
            RetainedSource::Recovery(generation) => Self::Recovery(generation),
            RetainedSource::Proposal => Self::Proposal {
                base: ProposalBase::Trusted,
            },
            RetainedSource::Remote(residency) => Self::Remote(residency),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkKind {
    Resolve,
    Verify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DirectKind {
    Local,
    TestAccept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AcceptedStatus {
    Pending,
    Gap,
    Proposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EvidenceContext {
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) witness: WitnessId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ResolvedEvidence {
    pub(super) context: EvidenceContext,
    pub(super) input_origins: BTreeMap<CellId, InputOrigin>,
    pub(super) dep_origins: BTreeMap<CellId, InputOrigin>,
    // Header dependencies are immutable chain-view reads. Keeping them in a
    // distinct set makes pool origin unrepresentable while still binding the
    // complete transaction footprint to this exact evidence cut.
    pub(super) header_deps: BTreeSet<HeaderId>,
}

impl ResolvedEvidence {
    pub(super) fn for_transaction(
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
    ) -> Self {
        Self {
            context: EvidenceContext {
                chain,
                rules,
                witness: transaction.witness,
            },
            input_origins: transaction
                .inputs
                .iter()
                .copied()
                .map(|cell| (cell, InputOrigin::Chain))
                .collect(),
            dep_origins: transaction
                .deps
                .iter()
                .copied()
                .map(|cell| (cell, InputOrigin::Chain))
                .collect(),
            header_deps: transaction.header_deps.clone(),
        }
    }

    pub(super) fn with_pool_input(
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
        cell: CellId,
        parent: TxId,
    ) -> Self {
        let mut evidence = Self::for_transaction(transaction, chain, rules);
        if evidence.input_origins.contains_key(&cell) {
            evidence
                .input_origins
                .insert(cell, InputOrigin::Pool(parent));
        }
        evidence
    }

    pub(super) fn is_for(
        &self,
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
    ) -> bool {
        self.context
            == (EvidenceContext {
                chain,
                rules,
                witness: transaction.witness,
            })
            && self.input_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.inputs
            && self.dep_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.deps
            && self.header_deps == transaction.header_deps
    }

    pub(super) fn has_transaction_shape(&self, transaction: &Transaction, rules: RulesId) -> bool {
        self.context.rules == rules
            && self.context.witness == transaction.witness
            && self.input_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.inputs
            && self.dep_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.deps
            && self.header_deps == transaction.header_deps
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkStage {
    Resolve,
    Verify(ResolvedEvidence),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkResult {
    Resolved(ResolvedEvidence),
    Verified,
    Missing(MissingDependencies),
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct MissingDependencies {
    transaction: TxId,
    cells: BTreeSet<CellId>,
    headers: BTreeSet<HeaderId>,
}

impl MissingDependencies {
    pub(super) fn for_transaction(
        transaction: &Transaction,
        cells: BTreeSet<CellId>,
    ) -> Option<Self> {
        Self::for_dependencies(transaction, cells, BTreeSet::new())
    }

    pub(super) fn for_headers(
        transaction: &Transaction,
        headers: BTreeSet<HeaderId>,
    ) -> Option<Self> {
        Self::for_dependencies(transaction, BTreeSet::new(), headers)
    }

    pub(super) fn for_dependencies(
        transaction: &Transaction,
        cells: BTreeSet<CellId>,
        headers: BTreeSet<HeaderId>,
    ) -> Option<Self> {
        let referenced = transaction
            .inputs
            .iter()
            .chain(&transaction.deps)
            .copied()
            .collect::<BTreeSet<_>>();
        (!(cells.is_empty() && headers.is_empty())
            && cells.is_subset(&referenced)
            && headers.is_subset(&transaction.header_deps))
        .then_some(Self {
            transaction: transaction.id,
            cells,
            headers,
        })
    }

    pub(super) fn is_for(&self, transaction: &Transaction) -> bool {
        self.transaction == transaction.id
            && !(self.cells.is_empty() && self.headers.is_empty())
            && self
                .cells
                .iter()
                .all(|cell| transaction.inputs.contains(cell) || transaction.deps.contains(cell))
            && self.headers.is_subset(&transaction.header_deps)
    }

    pub(super) fn cells(&self) -> &BTreeSet<CellId> {
        &self.cells
    }

    pub(super) fn headers(&self) -> &BTreeSet<HeaderId> {
        &self.headers
    }

    pub(super) fn has_headers(&self) -> bool {
        !self.headers.is_empty()
    }

    pub(super) fn extend(&mut self, transaction: &Transaction, cells: &BTreeSet<CellId>) -> bool {
        if self.transaction != transaction.id
            || cells
                .iter()
                .any(|cell| !transaction.inputs.contains(cell) && !transaction.deps.contains(cell))
        {
            return false;
        }
        self.cells.extend(cells.iter().copied());
        true
    }

    pub(super) fn retain_unavailable(&mut self, available: &BTreeSet<CellId>) {
        self.cells.retain(|cell| !available.contains(cell));
    }

    pub(super) fn retain_unavailable_headers(&mut self, available: &BTreeSet<HeaderId>) {
        self.headers.retain(|header| !available.contains(header));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.headers.is_empty()
    }
}

impl WorkStage {
    pub(super) const fn kind(&self) -> WorkKind {
        match self {
            Self::Resolve => WorkKind::Resolve,
            Self::Verify(_) => WorkKind::Verify,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RetainedPhase {
    Queued(WorkStage),
    Computing(WorkStage),
    Waiting { missing: MissingDependencies },
    Ready(ResolvedEvidence),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RetainedOwner {
    pub(super) source: Source,
    pub(super) phase: RetainedPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum OwnerLocation {
    Retained(RetainedOwner),
    Accepted {
        provenance: AcceptedProvenance,
        status: AcceptedStatus,
        accepted_at_wall: u64,
        evidence: ResolvedEvidence,
    },
    ReplacementHistory {
        missing: MissingDependencies,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Owner {
    pub(super) version: EntryVersion,
    pub(super) arrival: Arrival,
    pub(super) transaction: Transaction,
    pub(super) location: OwnerLocation,
}

impl Owner {
    pub(super) const fn retained_source(&self) -> Option<Source> {
        match &self.location {
            OwnerLocation::Retained(retained) => Some(retained.source),
            OwnerLocation::Accepted { .. } | OwnerLocation::ReplacementHistory { .. } => None,
        }
    }

    pub(super) const fn ingress_peer(&self) -> Option<PeerId> {
        match &self.location {
            OwnerLocation::Retained(retained) => retained.source.ingress_peer(),
            OwnerLocation::Accepted { provenance, .. } => provenance.ingress_peer(),
            OwnerLocation::ReplacementHistory { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum AcceptanceEffect {
    Admission {
        status: AcceptedStatus,
        ingress_peer: Option<PeerId>,
    },
    Duplicate {
        requesting_peer: Option<PeerId>,
    },
    ChainStatusChange {
        status: AcceptedStatus,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum MembershipRejection {
    Unavailable,
    Policy,
    Resource,
    CandidateEvicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RejectionEffect {
    Validation {
        ingress_peer: Option<PeerId>,
    },
    Membership {
        ingress_peer: Option<PeerId>,
        reason: MembershipRejection,
    },
    Replaced {
        winner: TxId,
    },
    CapacityEvicted,
    Expired,
    ChainConflict {
        cell: CellId,
        accepted: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum LogicalEffect {
    Accepted {
        transaction: TxId,
        payload_bytes: u32,
        cause: AcceptanceEffect,
    },
    Rejected {
        transaction: TxId,
        payload_bytes: u32,
        cause: RejectionEffect,
    },
    ChainCommitted(TxId),
    PeerCohortRevoked(PeerId),
    RemoteExpired(TxId),
    IngressReleased(TxId),
    ParentTransactionsRequested {
        transaction: TxId,
        parent_count: u16,
    },
    GenerationReset,
}

impl LogicalEffect {
    pub(super) const fn admitted(
        transaction: &Transaction,
        status: AcceptedStatus,
        ingress_peer: Option<PeerId>,
    ) -> Self {
        Self::Accepted {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: AcceptanceEffect::Admission {
                status,
                ingress_peer,
            },
        }
    }

    pub(super) const fn accepted_duplicate(
        transaction: TxId,
        requesting_peer: Option<PeerId>,
    ) -> Self {
        Self::Accepted {
            transaction,
            payload_bytes: 0,
            cause: AcceptanceEffect::Duplicate { requesting_peer },
        }
    }

    pub(super) const fn status_changed(transaction: &Transaction, status: AcceptedStatus) -> Self {
        Self::Accepted {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: AcceptanceEffect::ChainStatusChange { status },
        }
    }

    pub(super) const fn validation_rejected(
        transaction: &Transaction,
        ingress_peer: Option<PeerId>,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::Validation { ingress_peer },
        }
    }

    pub(super) const fn membership_rejected(
        transaction: &Transaction,
        ingress_peer: Option<PeerId>,
        reason: MembershipRejection,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::Membership {
                ingress_peer,
                reason,
            },
        }
    }

    pub(super) const fn replaced(transaction: &Transaction, winner: TxId) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::Replaced { winner },
        }
    }

    pub(super) const fn capacity_evicted(transaction: &Transaction) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::CapacityEvicted,
        }
    }

    pub(super) const fn expired(transaction: &Transaction) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::Expired,
        }
    }

    pub(super) const fn chain_conflict(
        transaction: &Transaction,
        cell: CellId,
        accepted: bool,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.bytes,
            cause: RejectionEffect::ChainConflict { cell, accepted },
        }
    }

    pub(super) fn parent_transactions_requested(
        transaction: TxId,
        parent_count: usize,
    ) -> Option<Self> {
        Some(Self::ParentTransactionsRequested {
            transaction,
            parent_count: u16::try_from(parent_count).ok()?,
        })
    }

    pub(super) fn charge_bytes(&self) -> Option<u32> {
        match self {
            Self::Accepted { payload_bytes, .. } | Self::Rejected { payload_bytes, .. } => {
                payload_bytes.checked_add(EFFECT_ENVELOPE_BYTES)
            }
            Self::ParentTransactionsRequested { parent_count, .. } => u32::from(*parent_count)
                .checked_mul(EFFECT_ID_BYTES)?
                .checked_add(EFFECT_ENVELOPE_BYTES),
            Self::ChainCommitted(_)
            | Self::PeerCohortRevoked(_)
            | Self::RemoteExpired(_)
            | Self::IngressReleased(_) => Some(EFFECT_ENVELOPE_BYTES),
            Self::GenerationReset => Some(0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectRecord {
    pub(super) stamp: ApplyStamp,
    pub(super) ordinal: u16,
    pub(super) logical: LogicalEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct WorkCapability {
    pub(super) id: CapabilityId,
    pub(super) transaction: TxId,
    pub(super) version: EntryVersion,
    pub(super) kind: WorkKind,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FinishedWorkCapability {
    pub(super) capability: WorkCapability,
    pub(super) result: WorkResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DirectCapability {
    pub(super) id: CapabilityId,
    pub(super) request: DirectRequestId,
    pub(super) kind: DirectKind,
    pub(super) transaction: Transaction,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectClaim {
    pub(super) stamp: ApplyStamp,
    pub(super) ordinal: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct AuthorityState {
    pub(super) generation: PoolGeneration,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) owners: BTreeMap<TxId, Owner>,
    pub(super) effects: VecDeque<EffectRecord>,
    pub(super) peer_bans: BTreeMap<PeerId, PeerBanRecord>,
    pub(super) last_apply: ApplyStamp,
    pub(super) next_version: u16,
    pub(super) next_arrival: u16,
    pub(super) limits: ModelLimits,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct LinearState {
    pub(super) work: BTreeMap<CapabilityId, WorkCapability>,
    pub(super) finished_work: BTreeMap<CapabilityId, FinishedWorkCapability>,
    pub(super) direct_work: BTreeMap<CapabilityId, DirectCapability>,
    pub(super) free_compute_permits: u16,
    pub(super) next_capability: u16,
    pub(super) effect_claim: Option<EffectClaim>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Omega {
    pub(super) authority: AuthorityState,
    pub(super) linear: LinearState,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ModelInvariantError {
    CounterOrder,
    OwnerKey,
    OwnerChargeOverflow,
    OwnerResourceLimit,
    RetainedResourceLimit,
    AcceptedResourceLimit,
    ReplacementHistoryResourceLimit,
    RemotePeerResourceLimit,
    DuplicateOwnerVersion,
    DuplicateProposalId,
    InvalidMissingDependencies,
    InvalidStoredEvidence,
    InvalidReplacementHistory,
    AcceptedDoubleSpend,
    DuplicateAcceptedOutput,
    StaleChainOrigin,
    AcceptedCausalCycle,
    MissingPoolParent,
    InvalidPoolParentOutput,
    ComputingWithoutCapability,
    DuplicateCurrentCapability,
    InvalidFinishedCapability,
    RetainedWorkerCapacity,
    CapabilityKey,
    DuplicateDirectRequest,
    CapabilityPermitConservation,
    EffectCapacity,
    EffectOrder,
    EffectClaim,
    PeerBanBound,
    PeerBanOrder,
}

impl Omega {
    pub(super) fn new(limits: ValidatedLimits, view: ViewId, rules: RulesId) -> Self {
        let limits = limits.get();
        Self {
            authority: AuthorityState {
                generation: PoolGeneration(0),
                chain: ChainView::initial(view),
                rules,
                owners: BTreeMap::new(),
                effects: VecDeque::new(),
                peer_bans: BTreeMap::new(),
                last_apply: ApplyStamp(0),
                next_version: 1,
                next_arrival: 1,
                limits,
            },
            linear: LinearState {
                work: BTreeMap::new(),
                finished_work: BTreeMap::new(),
                direct_work: BTreeMap::new(),
                free_compute_permits: limits.compute_permits,
                next_capability: 1,
                effect_claim: None,
            },
        }
    }

    pub(super) fn owner_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.authority
            .owners
            .values()
            .try_fold(ResourceVector::ZERO, |total, owner| {
                let charge = owner
                    .transaction
                    .charge()
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)?;
                total
                    .checked_add(charge)
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)
            })
    }

    pub(super) fn proposal_owner(&self, proposal: ProposalId) -> Option<TxId> {
        self.authority
            .owners
            .iter()
            .find_map(|(id, owner)| (owner.transaction.proposal == proposal).then_some(*id))
    }

    pub(super) fn retained_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| matches!(owner.location, OwnerLocation::Retained(_)))
    }

    pub(super) fn accepted_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| matches!(owner.location, OwnerLocation::Accepted { .. }))
    }

    pub(super) fn replacement_history_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| {
            matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
        })
    }

    pub(super) fn remote_peer_usage(
        &self,
        peer: PeerId,
    ) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| {
            owner.retained_source().and_then(Source::ingress_peer) == Some(peer)
        })
    }

    fn usage_matching(
        &self,
        include: impl Fn(&Owner) -> bool,
    ) -> Result<ResourceVector, ModelInvariantError> {
        self.authority
            .owners
            .values()
            .filter(|owner| include(owner))
            .try_fold(ResourceVector::ZERO, |total, owner| {
                let charge = owner
                    .transaction
                    .charge()
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)?;
                total
                    .checked_add(charge)
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)
            })
    }

    pub(super) fn effect_usage(&self) -> Option<(u16, u32)> {
        let records = u16::try_from(self.authority.effects.len()).ok()?;
        let bytes = self
            .authority
            .effects
            .iter()
            .try_fold(0u32, |used, effect| {
                used.checked_add(effect.logical.charge_bytes()?)
            })?;
        Some((records, bytes))
    }

    pub(super) fn check_invariants(&self) -> Result<(), ModelInvariantError> {
        let usage = self.owner_usage()?;
        if !usage.fits(self.authority.limits.owners) {
            return Err(ModelInvariantError::OwnerResourceLimit);
        }
        if !self.retained_usage()?.fits(self.authority.limits.retained) {
            return Err(ModelInvariantError::RetainedResourceLimit);
        }
        if !self.accepted_usage()?.fits(self.authority.limits.accepted) {
            return Err(ModelInvariantError::AcceptedResourceLimit);
        }
        if !self
            .replacement_history_usage()?
            .fits(self.authority.limits.replacement_history)
        {
            return Err(ModelInvariantError::ReplacementHistoryResourceLimit);
        }
        for peer in self
            .authority
            .owners
            .values()
            .filter_map(|owner| owner.retained_source().and_then(Source::ingress_peer))
            .collect::<BTreeSet<_>>()
        {
            if !self
                .remote_peer_usage(peer)?
                .fits(self.authority.limits.remote_per_peer)
            {
                return Err(ModelInvariantError::RemotePeerResourceLimit);
            }
        }

        let mut versions = BTreeSet::new();
        let mut proposals = BTreeSet::new();
        let mut spenders = BTreeMap::new();
        let mut accepted_producers = BTreeMap::new();
        for (id, owner) in &self.authority.owners {
            if !matches!(owner.location, OwnerLocation::Accepted { .. }) {
                continue;
            }
            for output in &owner.transaction.outputs {
                if accepted_producers.insert(*output, *id).is_some() {
                    return Err(ModelInvariantError::DuplicateAcceptedOutput);
                }
            }
        }
        for (id, owner) in &self.authority.owners {
            if *id != owner.transaction.id {
                return Err(ModelInvariantError::OwnerKey);
            }
            if !versions.insert(owner.version) {
                return Err(ModelInvariantError::DuplicateOwnerVersion);
            }
            if !proposals.insert(owner.transaction.proposal) {
                return Err(ModelInvariantError::DuplicateProposalId);
            }
            match &owner.location {
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Waiting { missing, .. },
                    ..
                }) if !missing.is_for(&owner.transaction) => {
                    return Err(ModelInvariantError::InvalidMissingDependencies);
                }
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(WorkStage::Verify(evidence)),
                    ..
                }) if !evidence.is_for(
                    &owner.transaction,
                    self.authority.chain,
                    self.authority.rules,
                ) =>
                {
                    return Err(ModelInvariantError::InvalidStoredEvidence);
                }
                OwnerLocation::Retained(RetainedOwner {
                    phase:
                        RetainedPhase::Queued(WorkStage::Verify(evidence))
                        | RetainedPhase::Ready(evidence),
                    ..
                }) if !evidence.has_transaction_shape(&owner.transaction, self.authority.rules) => {
                    return Err(ModelInvariantError::InvalidStoredEvidence);
                }
                OwnerLocation::Accepted { evidence, .. } => {
                    if !evidence.has_transaction_shape(&owner.transaction, self.authority.rules) {
                        return Err(ModelInvariantError::InvalidStoredEvidence);
                    }
                    for (cell, origin) in &evidence.input_origins {
                        if spenders.insert(*cell, *id).is_some() {
                            return Err(ModelInvariantError::AcceptedDoubleSpend);
                        }
                        match origin {
                            InputOrigin::Chain if accepted_producers.contains_key(cell) => {
                                return Err(ModelInvariantError::StaleChainOrigin);
                            }
                            InputOrigin::Pool(parent) => {
                                if parent == id {
                                    return Err(ModelInvariantError::AcceptedCausalCycle);
                                }
                                let Some(parent_owner) = self.authority.owners.get(parent) else {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                };
                                if !matches!(parent_owner.location, OwnerLocation::Accepted { .. })
                                {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                }
                                if accepted_producers.get(cell) != Some(parent)
                                    || !parent_owner.transaction.outputs.contains(cell)
                                {
                                    return Err(ModelInvariantError::InvalidPoolParentOutput);
                                }
                            }
                            InputOrigin::Chain => {}
                        }
                    }
                    for (cell, origin) in &evidence.dep_origins {
                        match origin {
                            InputOrigin::Chain if accepted_producers.contains_key(cell) => {
                                return Err(ModelInvariantError::StaleChainOrigin);
                            }
                            InputOrigin::Pool(parent) => {
                                if parent == id {
                                    return Err(ModelInvariantError::AcceptedCausalCycle);
                                }
                                let Some(parent_owner) = self.authority.owners.get(parent) else {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                };
                                if !matches!(parent_owner.location, OwnerLocation::Accepted { .. })
                                {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                }
                                if accepted_producers.get(cell) != Some(parent)
                                    || !parent_owner.transaction.outputs.contains(cell)
                                {
                                    return Err(ModelInvariantError::InvalidPoolParentOutput);
                                }
                            }
                            InputOrigin::Chain => {}
                        }
                    }
                }
                OwnerLocation::ReplacementHistory { missing }
                    if !missing.is_for(&owner.transaction) || missing.has_headers() =>
                {
                    return Err(ModelInvariantError::InvalidReplacementHistory);
                }
                _ => {}
            }
        }
        let mut accepted_indegree = BTreeMap::new();
        let mut accepted_children = BTreeMap::<TxId, BTreeSet<TxId>>::new();
        for (id, owner) in &self.authority.owners {
            let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                continue;
            };
            let parents = evidence
                .input_origins
                .values()
                .chain(evidence.dep_origins.values())
                .filter_map(|origin| match origin {
                    InputOrigin::Pool(parent) => Some(*parent),
                    InputOrigin::Chain => None,
                })
                .collect::<BTreeSet<_>>();
            accepted_indegree.insert(*id, parents.len());
            for parent in parents {
                accepted_children.entry(parent).or_default().insert(*id);
            }
        }
        let mut acyclic_frontier = accepted_indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut acyclic_count = 0usize;
        while let Some(parent) = acyclic_frontier.pop_front() {
            acyclic_count = acyclic_count
                .checked_add(1)
                .ok_or(ModelInvariantError::AcceptedCausalCycle)?;
            let Some(children) = accepted_children.get(&parent) else {
                continue;
            };
            for child in children {
                let Some(degree) = accepted_indegree.get_mut(child) else {
                    return Err(ModelInvariantError::MissingPoolParent);
                };
                *degree = degree
                    .checked_sub(1)
                    .ok_or(ModelInvariantError::AcceptedCausalCycle)?;
                if *degree == 0 {
                    acyclic_frontier.push_back(*child);
                }
            }
        }
        if acyclic_count != accepted_indegree.len() {
            return Err(ModelInvariantError::AcceptedCausalCycle);
        }
        if self.authority.owners.values().any(|owner| {
            owner.version.0 >= self.authority.next_version
                || owner.arrival.0 >= self.authority.next_arrival
        }) {
            return Err(ModelInvariantError::CounterOrder);
        }

        for owner in self.authority.owners.values() {
            let OwnerLocation::Retained(RetainedOwner {
                phase: RetainedPhase::Computing(stage),
                ..
            }) = &owner.location
            else {
                continue;
            };
            let current = self
                .linear
                .work
                .values()
                .chain(
                    self.linear
                        .finished_work
                        .values()
                        .map(|finished| &finished.capability),
                )
                .filter(|capability| {
                    capability.transaction == owner.transaction.id
                        && capability.version == owner.version
                        && capability.kind == stage.kind()
                        && capability.chain == self.authority.chain
                        && capability.rules == self.authority.rules
                })
                .count();
            match current {
                0 => return Err(ModelInvariantError::ComputingWithoutCapability),
                1 => {}
                _ => return Err(ModelInvariantError::DuplicateCurrentCapability),
            }
        }

        for (key, capability) in &self.linear.work {
            if *key != capability.id || capability.id.0 >= self.linear.next_capability {
                return Err(ModelInvariantError::CapabilityKey);
            }
            let Some(owner) = self.authority.owners.get(&capability.transaction) else {
                continue;
            };
            if owner.version != capability.version
                || capability.chain != self.authority.chain
                || capability.rules != self.authority.rules
            {
                continue;
            }
            if !matches!(
                &owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(stage),
                    ..
                }) if stage.kind() == capability.kind
            ) {
                return Err(ModelInvariantError::DuplicateCurrentCapability);
            }
        }

        for (key, finished) in &self.linear.finished_work {
            let capability = finished.capability;
            if *key != capability.id
                || capability.id.0 >= self.linear.next_capability
                || self.linear.work.contains_key(key)
            {
                return Err(ModelInvariantError::CapabilityKey);
            }
            let Some(owner) = self.authority.owners.get(&capability.transaction) else {
                continue;
            };
            if owner.version != capability.version
                || capability.chain != self.authority.chain
                || capability.rules != self.authority.rules
            {
                continue;
            }
            if !matches!(
                &owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(stage),
                    ..
                }) if stage.kind() == capability.kind
            ) {
                return Err(ModelInvariantError::InvalidFinishedCapability);
            }
        }

        let retained_worker_slots = self
            .linear
            .work
            .len()
            .checked_add(self.linear.finished_work.len())
            .ok_or(ModelInvariantError::RetainedWorkerCapacity)?;
        if retained_worker_slots > usize::from(self.authority.limits.compute_permits) {
            return Err(ModelInvariantError::RetainedWorkerCapacity);
        }

        let mut direct_requests = BTreeSet::new();
        for (key, capability) in &self.linear.direct_work {
            if *key != capability.id
                || capability.id.0 >= self.linear.next_capability
                || self.linear.work.contains_key(key)
                || self.linear.finished_work.contains_key(key)
            {
                return Err(ModelInvariantError::CapabilityKey);
            }
            if !direct_requests.insert(capability.request) {
                return Err(ModelInvariantError::DuplicateDirectRequest);
            }
            if capability.chain != self.authority.chain || capability.rules != self.authority.rules
            {
                continue;
            }
        }

        let held = self
            .linear
            .work
            .len()
            .checked_add(self.linear.direct_work.len())
            .and_then(|held| u16::try_from(held).ok())
            .ok_or(ModelInvariantError::CapabilityPermitConservation)?;
        if self.linear.free_compute_permits.checked_add(held)
            != Some(self.authority.limits.compute_permits)
        {
            return Err(ModelInvariantError::CapabilityPermitConservation);
        }

        let Some((records, bytes)) = self.effect_usage() else {
            return Err(ModelInvariantError::EffectCapacity);
        };
        if records > self.authority.limits.effect_records
            || bytes > self.authority.limits.effect_bytes
        {
            return Err(ModelInvariantError::EffectCapacity);
        }
        let mut previous = None;
        for effect in &self.authority.effects {
            let key = (effect.stamp, effect.ordinal);
            if effect.stamp.0 == 0
                || effect.stamp > self.authority.last_apply
                || previous.is_some_and(|previous_key| previous_key >= key)
            {
                return Err(ModelInvariantError::EffectOrder);
            }
            previous = Some(key);
        }
        if let Some(claim) = self.linear.effect_claim
            && self
                .authority
                .effects
                .front()
                .map(|effect| (effect.stamp, effect.ordinal))
                != Some((claim.stamp, claim.ordinal))
        {
            return Err(ModelInvariantError::EffectClaim);
        }
        if u16::try_from(self.authority.peer_bans.len())
            .ok()
            .is_none_or(|count| count > self.authority.limits.peer_ban_fences)
        {
            return Err(ModelInvariantError::PeerBanBound);
        }
        let mut ban_orders = BTreeSet::new();
        if self.authority.peer_bans.values().any(|record| {
            record.order > self.authority.last_apply || !ban_orders.insert(record.order)
        }) {
            return Err(ModelInvariantError::PeerBanOrder);
        }
        Ok(())
    }

    pub(super) fn ready_order(&self) -> Vec<TxId> {
        let mut ready = self
            .authority
            .owners
            .values()
            .filter_map(|owner| {
                matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Ready(_),
                        ..
                    })
                )
                .then_some((
                    owner.retained_source()?.priority(),
                    std::cmp::Reverse(owner.transaction.fee),
                    owner.arrival,
                    owner.transaction.id,
                ))
            })
            .collect::<Vec<_>>();
        ready.sort_unstable();
        ready.into_iter().map(|(_, _, _, id)| id).collect()
    }

    pub(super) fn queued_order(&self) -> Vec<TxId> {
        let mut queued = self
            .authority
            .owners
            .values()
            .filter_map(|owner| {
                matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Queued(_),
                        ..
                    })
                )
                .then_some((
                    owner.retained_source()?.priority(),
                    owner.arrival,
                    owner.transaction.id,
                ))
            })
            .collect::<Vec<_>>();
        queued.sort_unstable();
        queued.into_iter().map(|(_, _, id)| id).collect()
    }
}
