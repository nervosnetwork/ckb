use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
};

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

/// Exact semantic order of production's Ready frontier, represented in
/// strongest-first order for the reference model. `TxId` is the finite-domain
/// abstraction of the raw transaction hash, and `Transaction::bytes` is the
/// extraction of the verified serialized-byte metric at this cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReadyKey {
    pub(super) source_priority: u8,
    pub(super) fee: u64,
    pub(super) serialized_bytes: u32,
    pub(super) arrival: Arrival,
    pub(super) transaction: TxId,
    pub(super) version: EntryVersion,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let left_rate = u128::from(self.fee) * u128::from(other.serialized_bytes);
        let right_rate = u128::from(other.fee) * u128::from(self.serialized_bytes);
        self.source_priority
            .cmp(&other.source_priority)
            .then_with(|| right_rate.cmp(&left_rate))
            .then_with(|| other.fee.cmp(&self.fee))
            .then_with(|| self.arrival.cmp(&other.arrival))
            .then_with(|| self.transaction.cmp(&other.transaction))
            .then_with(|| other.version.cmp(&self.version))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

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

/// Semantic publication class for one immutable committed-effect batch.
///
/// The class is selected by the command authority, not reconstructed from an
/// effect's audience: a Proposal promoted from Remote remains attributable to
/// its peer while consuming trusted publication headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum EffectClass {
    Remote,
    Trusted,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct EffectCapacity {
    pub(super) batches: u16,
    pub(super) bytes: u32,
}

impl EffectCapacity {
    pub(super) const fn new(batches: u16, bytes: u32) -> Self {
        Self { batches, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(other.batches)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.batches <= limit.batches && self.bytes <= limit.bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectBatchBound {
    pub(super) effects: u16,
    pub(super) bytes: u32,
}

impl EffectBatchBound {
    pub(super) const fn new(effects: u16, bytes: u32) -> Self {
        Self { effects, bytes }
    }
}

/// Partitioned committed-effect capacity. `trusted_headroom` extends the
/// ordinary region without being available to Remote work; critical headroom
/// extends only the total region. This is the mathematical counterpart of the
/// production journal's three nested regions, not a second scheduling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectLimits {
    pub(super) remote: EffectCapacity,
    pub(super) trusted_headroom: EffectCapacity,
    pub(super) critical_headroom: EffectCapacity,
    pub(super) remote_bound: EffectBatchBound,
    pub(super) trusted_bound: EffectBatchBound,
    pub(super) critical_bound: EffectBatchBound,
}

impl EffectLimits {
    pub(super) const fn small() -> Self {
        Self {
            remote: EffectCapacity::new(8, 1_536),
            trusted_headroom: EffectCapacity::new(4, 768),
            critical_headroom: EffectCapacity::new(1, 192),
            remote_bound: EffectBatchBound::new(5, 192),
            trusted_bound: EffectBatchBound::new(5, 192),
            critical_bound: EffectBatchBound::new(5, 192),
        }
    }

    pub(super) const fn bound(self, class: EffectClass) -> EffectBatchBound {
        match class {
            EffectClass::Remote => self.remote_bound,
            EffectClass::Trusted => self.trusted_bound,
            EffectClass::Critical => self.critical_bound,
        }
    }

    fn ordinary(self) -> Option<EffectCapacity> {
        self.remote.checked_add(self.trusted_headroom)
    }

    fn total(self) -> Option<EffectCapacity> {
        self.ordinary()?.checked_add(self.critical_headroom)
    }

    fn validates(self, largest_effects: u16, largest_bytes: u32) -> bool {
        let Some(ordinary) = self.ordinary() else {
            return false;
        };
        let Some(total) = self.total() else {
            return false;
        };
        if self.remote.batches == 0
            || self.remote.bytes == 0
            || ordinary.batches == 0
            || ordinary.bytes == 0
            || total.batches == 0
            || total.bytes == 0
        {
            return false;
        }
        [self.remote_bound, self.trusted_bound, self.critical_bound]
            .into_iter()
            .all(|bound| {
                bound.effects >= largest_effects
                    && bound.bytes >= largest_bytes
                    && bound.effects != 0
                    && bound.bytes != 0
            })
            && self.remote_bound.bytes <= self.remote.bytes
            && self.trusted_bound.bytes <= ordinary.bytes
            && self.critical_bound.bytes <= total.bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct EffectUsage {
    pub(super) remote: EffectCapacity,
    pub(super) ordinary: EffectCapacity,
    pub(super) total: EffectCapacity,
}

impl EffectUsage {
    fn checked_charge(self, class: EffectClass, bytes: u32) -> Option<Self> {
        let batch = EffectCapacity::new(1, bytes);
        Some(match class {
            EffectClass::Remote => Self {
                remote: self.remote.checked_add(batch)?,
                ordinary: self.ordinary.checked_add(batch)?,
                total: self.total.checked_add(batch)?,
            },
            EffectClass::Trusted => Self {
                ordinary: self.ordinary.checked_add(batch)?,
                total: self.total.checked_add(batch)?,
                ..self
            },
            EffectClass::Critical => Self {
                total: self.total.checked_add(batch)?,
                ..self
            },
        })
    }

    fn fits(self, limits: EffectLimits) -> bool {
        let Some(ordinary) = limits.ordinary() else {
            return false;
        };
        let Some(total) = limits.total() else {
            return false;
        };
        self.remote.fits(limits.remote) && self.ordinary.fits(ordinary) && self.total.fits(total)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ModelLimits {
    pub(super) owners: ResourceVector,
    pub(super) retained: ResourceVector,
    pub(super) accepted: ResourceVector,
    pub(super) replacement_history: ResourceVector,
    pub(super) remote_per_peer: ResourceVector,
    pub(super) effects: EffectLimits,
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
            effects: EffectLimits::small(),
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
        if !self
            .effects
            .validates(largest_effect_batch_records, largest_effect_batch_bytes)
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

    /// Effect control follows the current command authority. Immutable peer
    /// attribution is deliberately not used for capacity selection.
    pub(super) const fn effect_class(self) -> EffectClass {
        match self {
            Self::Remote(_) => EffectClass::Remote,
            Self::Recovery(_) | Self::Proposal { .. } => EffectClass::Trusted,
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
    pub(super) class: EffectClass,
    /// Immutable batch metadata is repeated on each logical record so partial
    /// endpoint progress cannot release journal capacity before the batch's
    /// final record settles.
    pub(super) batch_effects: u16,
    pub(super) batch_bytes: u32,
    pub(super) logical: LogicalEffect,
}

impl EffectRecord {
    pub(super) fn batch_shape(effects: &[LogicalEffect]) -> Option<(u16, u32)> {
        let count = u16::try_from(effects.len()).ok()?;
        let bytes = effects.iter().try_fold(0u32, |used, effect| {
            used.checked_add(effect.charge_bytes()?)
        })?;
        Some((count, bytes))
    }

    /// Sealed model constructor for one immutable publication batch.
    ///
    /// Repeating the full batch shape on every remaining record is what keeps
    /// a partially consumed batch charged as one indivisible journal unit.
    pub(super) fn from_batch(
        stamp: ApplyStamp,
        class: EffectClass,
        effects: Vec<LogicalEffect>,
    ) -> Option<Vec<Self>> {
        if effects.is_empty()
            || effects
                .iter()
                .any(|effect| matches!(effect, LogicalEffect::GenerationReset))
                && !(class == EffectClass::Critical
                    && matches!(effects.as_slice(), [LogicalEffect::GenerationReset]))
        {
            return None;
        }
        let (batch_effects, batch_bytes) = Self::batch_shape(&effects)?;
        effects
            .into_iter()
            .enumerate()
            .map(|(ordinal, logical)| {
                Some(Self {
                    stamp,
                    ordinal: u16::try_from(ordinal).ok()?,
                    class,
                    batch_effects,
                    batch_bytes,
                    logical,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum EffectClaimSource {
    Queued,
    GenerationReset,
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
    pub(super) source: EffectClaimSource,
    pub(super) stamp: ApplyStamp,
    pub(super) ordinal: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct AuthorityState {
    pub(super) generation: PoolGeneration,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) owners: BTreeMap<TxId, Owner>,
    /// Capacity-charged immutable publication batches in commit order.
    pub(super) effects: VecDeque<EffectRecord>,
    /// Rebuildable reset publication. A newer reset subsumes the previous
    /// reset without consuming charged journal capacity.
    pub(super) latest_generation_reset: Option<EffectRecord>,
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
                latest_generation_reset: None,
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

    pub(super) fn effect_batch_shape(effects: &[LogicalEffect]) -> Option<(u16, u32)> {
        EffectRecord::batch_shape(effects)
    }

    pub(super) fn can_append_effects(&self, class: EffectClass, effects: &[LogicalEffect]) -> bool {
        if effects.is_empty() {
            return true;
        }
        if !self.can_append_effects_against_empty_bound(class, effects) {
            return false;
        }
        if matches!(effects, [LogicalEffect::GenerationReset]) {
            return true;
        }
        let Some((_, bytes)) = Self::effect_batch_shape(effects) else {
            return false;
        };
        self.effect_usage()
            .and_then(|usage| usage.checked_charge(class, bytes))
            .is_some_and(|usage| usage.fits(self.authority.limits.effects))
    }

    /// Injects one otherwise valid committed batch for capacity-bound model
    /// fixtures. Tests use this instead of hand-building journal records, so
    /// batch metadata and stamp ordering cannot drift from the model law.
    pub(super) fn append_effect_fixture(
        &mut self,
        class: EffectClass,
        effects: Vec<LogicalEffect>,
    ) -> bool {
        if effects.is_empty() || !self.can_append_effects(class, &effects) {
            return false;
        }
        let Some(stamp) = self.authority.last_apply.0.checked_add(1).map(ApplyStamp) else {
            return false;
        };
        let Some(records) = EffectRecord::from_batch(stamp, class, effects) else {
            return false;
        };
        if !self.install_effect_records(records) {
            return false;
        }
        self.authority.last_apply = stamp;
        true
    }

    fn can_append_effects_against_empty_bound(
        &self,
        class: EffectClass,
        effects: &[LogicalEffect],
    ) -> bool {
        if matches!(effects, [LogicalEffect::GenerationReset]) {
            return class == EffectClass::Critical;
        }
        if effects.is_empty()
            || effects
                .iter()
                .any(|effect| matches!(effect, LogicalEffect::GenerationReset))
        {
            return effects.is_empty();
        }
        let Some((count, bytes)) = Self::effect_batch_shape(effects) else {
            return false;
        };
        let bound = self.authority.limits.effects.bound(class);
        count <= bound.effects && bytes <= bound.bytes
    }

    pub(super) fn effect_usage(&self) -> Option<EffectUsage> {
        let mut usage = EffectUsage::default();
        let mut previous_batch = None;
        for effect in &self.authority.effects {
            if previous_batch == Some(effect.stamp) {
                continue;
            }
            previous_batch = Some(effect.stamp);
            usage = usage.checked_charge(effect.class, effect.batch_bytes)?;
        }
        Some(usage)
    }

    pub(super) fn next_effect_record(&self) -> Option<(EffectClaimSource, &EffectRecord)> {
        match (
            self.authority.effects.front(),
            self.authority.latest_generation_reset.as_ref(),
        ) {
            (Some(queued), Some(reset)) if reset.stamp < queued.stamp => {
                Some((EffectClaimSource::GenerationReset, reset))
            }
            (Some(queued), _) => Some((EffectClaimSource::Queued, queued)),
            (None, Some(reset)) => Some((EffectClaimSource::GenerationReset, reset)),
            (None, None) => None,
        }
    }

    pub(super) fn has_pending_effects(&self) -> bool {
        !self.authority.effects.is_empty() || self.authority.latest_generation_reset.is_some()
    }

    pub(super) fn install_effect_records(&mut self, mut records: Vec<EffectRecord>) -> bool {
        let Some(first) = records.first() else {
            return false;
        };
        let latest_stamp = self
            .authority
            .effects
            .back()
            .map(|effect| effect.stamp)
            .into_iter()
            .chain(
                self.authority
                    .latest_generation_reset
                    .as_ref()
                    .map(|reset| reset.stamp),
            )
            .max();
        if latest_stamp.is_some_and(|latest| latest >= first.stamp) {
            return false;
        }
        if matches!(first.logical, LogicalEffect::GenerationReset) {
            if records.len() != 1 {
                return false;
            }
            let Some(reset) = records.pop() else {
                return false;
            };
            self.authority.latest_generation_reset = Some(reset);
        } else {
            self.authority.effects.extend(records);
        }
        true
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

        let Some(effect_usage) = self.effect_usage() else {
            return Err(ModelInvariantError::EffectCapacity);
        };
        if !effect_usage.fits(self.authority.limits.effects) {
            return Err(ModelInvariantError::EffectCapacity);
        }
        let mut previous = None;
        let effects = self.authority.effects.iter().collect::<Vec<_>>();
        for effect in &effects {
            let key = (effect.stamp, effect.ordinal);
            if effect.stamp.0 == 0
                || effect.stamp > self.authority.last_apply
                || previous.is_some_and(|previous_key| previous_key >= key)
            {
                return Err(ModelInvariantError::EffectOrder);
            }
            previous = Some(key);
        }
        let mut batch_start = 0usize;
        while batch_start < effects.len() {
            let first = effects[batch_start];
            let mut batch_end = batch_start + 1;
            while batch_end < effects.len() && effects[batch_end].stamp == first.stamp {
                batch_end += 1;
            }
            let batch = &effects[batch_start..batch_end];
            let remaining_bytes = batch.iter().try_fold(0u32, |used, record| {
                used.checked_add(record.logical.charge_bytes()?)
            });
            let Some(remaining_bytes) = remaining_bytes else {
                return Err(ModelInvariantError::EffectCapacity);
            };
            let bound = self.authority.limits.effects.bound(first.class);
            let last_ordinal = batch.last().map(|record| record.ordinal);
            if first.ordinal > 0 && batch_start != 0
                || batch.iter().enumerate().any(|(offset, record)| {
                    u16::try_from(offset)
                        .ok()
                        .and_then(|offset| first.ordinal.checked_add(offset))
                        != Some(record.ordinal)
                        || record.class != first.class
                        || record.batch_effects != first.batch_effects
                        || record.batch_bytes != first.batch_bytes
                })
                || last_ordinal.and_then(|ordinal| ordinal.checked_add(1))
                    != Some(first.batch_effects)
                || first.batch_effects > bound.effects
                || first.batch_bytes > bound.bytes
                || remaining_bytes > first.batch_bytes
                || first.ordinal == 0
                    && (u16::try_from(batch.len()).ok() != Some(first.batch_effects)
                        || remaining_bytes != first.batch_bytes)
                || matches!(first.logical, LogicalEffect::GenerationReset)
            {
                return Err(ModelInvariantError::EffectOrder);
            }
            batch_start = batch_end;
        }
        if let Some(reset) = &self.authority.latest_generation_reset
            && (reset.stamp.0 == 0
                || reset.stamp > self.authority.last_apply
                || reset.ordinal != 0
                || reset.class != EffectClass::Critical
                || reset.batch_effects != 1
                || reset.batch_bytes != 0
                || !matches!(reset.logical, LogicalEffect::GenerationReset)
                || effects.iter().any(|effect| effect.stamp == reset.stamp))
        {
            return Err(ModelInvariantError::EffectOrder);
        }
        if let Some(claim) = self.linear.effect_claim {
            let valid = match claim.source {
                EffectClaimSource::Queued => self.authority.effects.front().is_some_and(|effect| {
                    (effect.stamp, effect.ordinal) == (claim.stamp, claim.ordinal)
                }),
                EffectClaimSource::GenerationReset => {
                    self.authority
                        .latest_generation_reset
                        .as_ref()
                        .is_some_and(|reset| {
                            claim.ordinal == 0 && reset.ordinal == 0 && reset.stamp >= claim.stamp
                        })
                        && self
                            .authority
                            .effects
                            .front()
                            .is_none_or(|queued| queued.stamp > claim.stamp)
                }
            };
            if !valid {
                return Err(ModelInvariantError::EffectClaim);
            }
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
                .then_some(ReadyKey {
                    source_priority: owner.retained_source()?.priority(),
                    fee: owner.transaction.fee,
                    serialized_bytes: owner.transaction.bytes,
                    arrival: owner.arrival,
                    transaction: owner.transaction.id,
                    version: owner.version,
                })
            })
            .collect::<Vec<_>>();
        ready.sort_unstable();
        ready.into_iter().map(|key| key.transaction).collect()
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
