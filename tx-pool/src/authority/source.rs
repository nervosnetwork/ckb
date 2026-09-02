//! Monotonic source versions compiled from authoritative owner transitions.
//!
//! These are not independent clocks and callers never publish template dirty
//! flags. Each field records the committed [`ApplySequence`] that last changed
//! the corresponding fact. The exhaustive before/after compiler below is the
//! only place that maps an owner transition to template work.

use super::shard::AUTHORITY_SHARD_COUNT;
use super::state::{
    AcceptedStatus, ApplySequence, ObservedDependencies, OwnedTx, PreAcceptedPhase,
    PreAcceptedSource, RemoteResidencyLease,
};
use ckb_util::parking_lot::Mutex;

/// Exact Accepted-selection identity.
///
/// `barrier` covers exclusive lifecycle/chain-era transitions. Ordinary
/// shared Apply never advances it: each physical owner shard advances only
/// its own counter at the actual commit linearization point. The fixed vector
/// therefore distinguishes reverse completion without introducing a global
/// commit lock, clock or hash collision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateSelectionSource {
    barrier: ApplySequence,
    shards: [u64; AUTHORITY_SHARD_COUNT],
}

impl TemplateSelectionSource {
    const fn initial() -> Self {
        Self {
            barrier: ApplySequence(0),
            shards: [0; AUTHORITY_SHARD_COUNT],
        }
    }

    pub(in crate::authority) const fn from_barrier(barrier: ApplySequence) -> Self {
        Self {
            barrier,
            shards: [0; AUTHORITY_SHARD_COUNT],
        }
    }

    pub(in crate::authority) fn with_shards(
        mut self,
        shards: [u64; AUTHORITY_SHARD_COUNT],
    ) -> Self {
        self.shards = shards;
        self
    }

    pub(super) fn join(self, incoming: Self) -> Self {
        let mut shards = self.shards;
        for (current, incoming) in shards.iter_mut().zip(incoming.shards) {
            *current = (*current).max(incoming);
        }
        Self {
            barrier: self.barrier.max(incoming.barrier),
            shards,
        }
    }

    pub(super) fn covers(self, target: Self) -> bool {
        self.barrier >= target.barrier
            && self
                .shards
                .iter()
                .zip(target.shards)
                .all(|(current, target)| *current >= target)
    }

    pub(in crate::authority) fn barrier(self) -> ApplySequence {
        self.barrier
    }

    fn advance_barrier(&mut self, sequence: ApplySequence) {
        self.barrier = self.barrier.max(sequence);
    }

    #[cfg(test)]
    pub(in crate::authority) fn compact_barrier_for_foundation(
        mut self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> Self {
        if self.barrier >= batch && self.barrier < canonical_next {
            self.barrier = batch;
        }
        self
    }
}

/// Source cut captured with accepted payloads for block-template work.
///
/// `proposals` and `transactions` are exact derived selection sources, so a
/// Gap/Proposed change does not cause unrelated proposal work and a
/// Pending/Gap change does not cause unrelated transaction work. Direct
/// validation carries an exact bounded Accepted read receipt instead of a
/// global content clock. Block-assembler configuration is immutable after
/// construction; chain-dependent policy is covered by `chain`, so there is no
/// producerless policy clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PoolTemplateVersions {
    pub(super) proposals: TemplateSelectionSource,
    pub(super) transactions: TemplateSelectionSource,
    pub(super) chain: ApplySequence,
}

impl PoolTemplateVersions {
    const fn initial() -> Self {
        Self {
            proposals: TemplateSelectionSource::initial(),
            transactions: TemplateSelectionSource::initial(),
            chain: ApplySequence(0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct AuthoritySourceVersionSnapshot {
    template: PoolTemplateVersions,
}

#[derive(Debug)]
pub(super) struct AuthoritySourceVersions {
    state: Mutex<AuthoritySourceVersionSnapshot>,
}

impl AuthoritySourceVersions {
    pub(super) fn initial() -> Self {
        Self {
            state: Mutex::new(AuthoritySourceVersionSnapshot {
                template: PoolTemplateVersions::initial(),
            }),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn lock_for_foundation(
        &self,
    ) -> ckb_util::parking_lot::MutexGuard<'_, AuthoritySourceVersionSnapshot> {
        self.state.lock()
    }

    pub(super) fn template(&self) -> PoolTemplateVersions {
        self.state.lock().template
    }

    pub(in crate::authority) fn snapshot(&self) -> AuthoritySourceVersionSnapshot {
        *self.state.lock()
    }

    #[cfg(test)]
    pub(in crate::authority) fn snapshot_with_template(
        &self,
        template: PoolTemplateVersions,
    ) -> AuthoritySourceVersionSnapshot {
        let mut snapshot = self.snapshot();
        snapshot.template.proposals = snapshot.template.proposals.join(template.proposals);
        snapshot.template.transactions = snapshot.template.transactions.join(template.transactions);
        snapshot
    }

    pub(super) fn plan_replacements<'entry>(
        &self,
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let impact = replacements
            .into_iter()
            .fold(SourceImpact::None, |impact, (before, after)| {
                impact.join(SourceImpact::for_replacement(before, after))
            });
        let before = self.snapshot();
        let after = before.with_impact(impact, sequence);
        SourceVersionDelta::between(before, after)
    }

    /// Whether one owner transition changes the relayer's missing-parent
    /// projection. Shared Apply uses this semantic classifier to advance the
    /// owning shard's local source counter in the same physical cut as the
    /// owner and deadline rows.
    pub(in crate::authority) fn relay_parent_change(
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> bool {
        relay_parent_projection(before) != relay_parent_projection(after)
    }

    /// Compile only Accepted selection sources from the transition itself.
    /// Monotonic per-shard maxima need no read of the global chain/relay
    /// source cell; callers must reject the route if another source can move.
    pub(super) fn plan_template_selection_replacements<'entry>(
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let impact = replacements
            .into_iter()
            .fold(SourceImpact::None, |impact, (before, after)| {
                impact.join(SourceImpact::for_replacement(before, after))
            });
        let mut delta = SourceVersionDelta {
            proposals: None,
            transactions: None,
            chain: None,
        };
        match impact {
            SourceImpact::None => {}
            SourceImpact::Status(TemplateSelectionImpact::Proposals) => {
                delta.proposals = Some(sequence);
            }
            SourceImpact::Status(TemplateSelectionImpact::Transactions) => {
                delta.transactions = Some(sequence);
            }
            SourceImpact::Status(TemplateSelectionImpact::Both) | SourceImpact::Accepted => {
                delta.proposals = Some(sequence);
                delta.transactions = Some(sequence);
            }
        }
        delta
    }

    /// Exact selection effect of one owner replacement. Shared Apply uses
    /// this same exhaustive compiler under its locked owner cut to advance
    /// per-shard sources by the number of semantic changes, not by the number
    /// or planning order of batches.
    pub(in crate::authority) fn template_selection_change(
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> (bool, bool) {
        match SourceImpact::for_replacement(before, after) {
            SourceImpact::None => (false, false),
            SourceImpact::Status(TemplateSelectionImpact::Proposals) => (true, false),
            SourceImpact::Status(TemplateSelectionImpact::Transactions) => (false, true),
            SourceImpact::Status(TemplateSelectionImpact::Both) | SourceImpact::Accepted => {
                (true, true)
            }
        }
    }

    /// A chain transition changes the template chain source even when its
    /// accepted owner set is unchanged. Owner-derived selection sources still
    /// advance only according to the same exhaustive transition compiler.
    pub(super) fn plan_chain_replacements<'entry>(
        &self,
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let mut delta = self.plan_replacements(replacements, sequence);
        delta.chain = Some(sequence);
        delta
    }

    pub(super) fn apply(&self, delta: SourceVersionDelta) {
        if delta.is_empty() {
            return;
        }
        let mut state = self.state.lock();
        if let Some(sequence) = delta.proposals {
            state.template.proposals.advance_barrier(sequence);
        }
        if let Some(sequence) = delta.transactions {
            state.template.transactions.advance_barrier(sequence);
        }
        if let Some(sequence) = delta.chain {
            state.template.chain = state.template.chain.max(sequence);
        }
    }

    pub(super) fn plan_generation_replacement(
        &self,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        SourceVersionDelta {
            proposals: Some(sequence),
            transactions: Some(sequence),
            chain: Some(sequence),
        }
    }
}

impl AuthoritySourceVersionSnapshot {
    #[cfg(test)]
    pub(in crate::authority) fn template(self) -> PoolTemplateVersions {
        self.template
    }

    fn with_impact(self, impact: SourceImpact, sequence: ApplySequence) -> Self {
        match impact {
            SourceImpact::None => self,
            SourceImpact::Status(selection) => {
                let mut template = self.template;
                selection.advance(&mut template, sequence);
                Self { template }
            }
            SourceImpact::Accepted => Self {
                template: PoolTemplateVersions {
                    proposals: TemplateSelectionSource::from_barrier(sequence),
                    transactions: TemplateSelectionSource::from_barrier(sequence),
                    ..self.template
                },
            },
        }
    }
}

#[cfg(test)]
mod monotonic_source_version_tests {
    use super::*;

    #[test]
    fn out_of_order_commits_cannot_regress_a_reserved_source_version() {
        let versions = AuthoritySourceVersions::initial();
        let older = versions.plan_generation_replacement(ApplySequence(10));
        let newer = versions.plan_generation_replacement(ApplySequence(11));

        versions.apply(newer);
        versions.apply(older);

        assert_eq!(
            versions.template(),
            PoolTemplateVersions {
                proposals: TemplateSelectionSource::from_barrier(ApplySequence(11)),
                transactions: TemplateSelectionSource::from_barrier(ApplySequence(11)),
                chain: ApplySequence(11),
            }
        );
    }
}

fn relay_parent_projection(
    owner: Option<&OwnedTx>,
) -> Option<(RemoteResidencyLease, &ObservedDependencies)> {
    let OwnedTx::PreAccepted(entry) = owner? else {
        return None;
    };
    let PreAcceptedSource::Remote(remote) = entry.source else {
        return None;
    };
    let PreAcceptedPhase::Waiting(observed) = &entry.phase else {
        return None;
    };
    Some((remote.residency, observed))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceVersionDelta {
    proposals: Option<ApplySequence>,
    transactions: Option<ApplySequence>,
    chain: Option<ApplySequence>,
}

impl SourceVersionDelta {
    pub(super) const fn empty() -> Self {
        Self {
            proposals: None,
            transactions: None,
            chain: None,
        }
    }

    pub(super) fn take_template_selection(&mut self) -> (bool, bool) {
        (
            self.proposals.take().is_some(),
            self.transactions.take().is_some(),
        )
    }

    pub(super) fn is_empty(&self) -> bool {
        self.proposals.is_none() && self.transactions.is_none() && self.chain.is_none()
    }

    fn between(
        before: AuthoritySourceVersionSnapshot,
        after: AuthoritySourceVersionSnapshot,
    ) -> Self {
        Self {
            proposals: (before.template.proposals != after.template.proposals)
                .then_some(after.template.proposals.barrier),
            transactions: (before.template.transactions != after.template.transactions)
                .then_some(after.template.transactions.barrier),
            chain: (before.template.chain != after.template.chain).then_some(after.template.chain),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateSelectionImpact {
    Proposals,
    Transactions,
    Both,
}

impl TemplateSelectionImpact {
    fn join(self, incoming: Self) -> Self {
        match (self, incoming) {
            (Self::Proposals, Self::Proposals) => Self::Proposals,
            (Self::Transactions, Self::Transactions) => Self::Transactions,
            (Self::Proposals | Self::Transactions | Self::Both, Self::Both)
            | (Self::Both, Self::Proposals | Self::Transactions)
            | (Self::Proposals, Self::Transactions)
            | (Self::Transactions, Self::Proposals) => Self::Both,
        }
    }

    fn advance(self, versions: &mut PoolTemplateVersions, sequence: ApplySequence) {
        match self {
            Self::Proposals => versions.proposals.advance_barrier(sequence),
            Self::Transactions => versions.transactions.advance_barrier(sequence),
            Self::Both => {
                versions.proposals.advance_barrier(sequence);
                versions.transactions.advance_barrier(sequence);
            }
        }
    }

    fn for_status_change(before: AcceptedStatus, after: AcceptedStatus) -> Option<Self> {
        match (before, after) {
            (AcceptedStatus::Pending, AcceptedStatus::Gap)
            | (AcceptedStatus::Gap, AcceptedStatus::Pending) => Some(Self::Proposals),
            (AcceptedStatus::Gap, AcceptedStatus::Proposed)
            | (AcceptedStatus::Proposed, AcceptedStatus::Gap) => Some(Self::Transactions),
            (AcceptedStatus::Pending, AcceptedStatus::Proposed)
            | (AcceptedStatus::Proposed, AcceptedStatus::Pending) => Some(Self::Both),
            (AcceptedStatus::Pending, AcceptedStatus::Pending)
            | (AcceptedStatus::Gap, AcceptedStatus::Gap)
            | (AcceptedStatus::Proposed, AcceptedStatus::Proposed) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceImpact {
    None,
    Status(TemplateSelectionImpact),
    Accepted,
}

impl SourceImpact {
    fn join(self, incoming: Self) -> Self {
        match (self, incoming) {
            (Self::Accepted, _) | (_, Self::Accepted) => Self::Accepted,
            (Self::Status(left), Self::Status(right)) => Self::Status(left.join(right)),
            (Self::Status(impact), Self::None) | (Self::None, Self::Status(impact)) => {
                Self::Status(impact)
            }
            (Self::None, Self::None) => Self::None,
        }
    }

    fn for_replacement(before: Option<&OwnedTx>, after: Option<&OwnedTx>) -> Self {
        match (before, after) {
            (Some(OwnedTx::Accepted(before)), Some(OwnedTx::Accepted(after))) => {
                // EntryVersion is OCC machinery, not an accepted fact. Every
                // other immutable record field participates in the source
                // cut so future metadata cannot masquerade as status-only.
                if before.record.identity != after.record.identity
                    || before.record.arrival != after.record.arrival
                    || before.accepted_at != after.accepted_at
                    || before.provenance != after.provenance
                    || before.proof != after.proof
                {
                    Self::Accepted
                } else if let Some(impact) =
                    TemplateSelectionImpact::for_status_change(before.status(), after.status())
                {
                    Self::Status(impact)
                } else {
                    Self::None
                }
            }
            (
                Some(OwnedTx::Accepted(_)),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
            )
            | (
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
                Some(OwnedTx::Accepted(_)),
            ) => Self::Accepted,
            (None, None) => Self::None,
            (
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
            )
            | (None, Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_))) => Self::None,
        }
    }
}

#[cfg(test)]
#[path = "tests/support/source.rs"]
pub(super) mod test_support;
