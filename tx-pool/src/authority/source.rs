//! Monotonic source versions compiled from authoritative owner transitions.
//!
//! These are not independent clocks and callers never publish template dirty
//! flags. Each field records the committed [`ApplySequence`] that last changed
//! the corresponding fact. The exhaustive before/after compiler below is the
//! only place that maps an owner transition to template work.

use super::state::{
    AcceptedStatus, ApplySequence, ObservedDependencies, OwnedTx, PreAcceptedPhase,
    PreAcceptedSource, RemoteResidencyLease,
};
use ckb_util::parking_lot::Mutex;

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
    pub(super) proposals: ApplySequence,
    pub(super) transactions: ApplySequence,
    pub(super) chain: ApplySequence,
}

impl PoolTemplateVersions {
    const fn initial() -> Self {
        Self {
            proposals: ApplySequence(0),
            transactions: ApplySequence(0),
            chain: ApplySequence(0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct AuthoritySourceVersionSnapshot {
    relay_parents: ApplySequence,
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
                relay_parents: ApplySequence(0),
                template: PoolTemplateVersions::initial(),
            }),
        }
    }

    /// Exact source for rebuilding the relayer's Remote missing-parent level.
    /// Effect settlement and unrelated owner transitions must not invalidate a
    /// bounded rebuild cursor; every relevant transition is compiled below.
    pub(super) fn relay_parents(&self) -> ApplySequence {
        self.state.lock().relay_parents
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
        snapshot.template.proposals = snapshot.template.proposals.max(template.proposals);
        snapshot.template.transactions = snapshot.template.transactions.max(template.transactions);
        snapshot
    }

    pub(super) fn plan_replacements<'entry>(
        &self,
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let (impact, relay_parents_changed) = replacements.into_iter().fold(
            (SourceImpact::None, false),
            |(impact, relay_parents_changed), (before, after)| {
                (
                    impact.join(SourceImpact::for_replacement(before, after)),
                    relay_parents_changed
                        || relay_parent_projection(before) != relay_parent_projection(after),
                )
            },
        );
        let before = self.snapshot();
        let after = before.with_impact(impact, sequence);
        let mut delta = SourceVersionDelta::between(before, after);
        if relay_parents_changed {
            delta.relay_parents = Some(sequence);
        }
        delta
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
            relay_parents: None,
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
        if let Some(sequence) = delta.relay_parents {
            state.relay_parents = state.relay_parents.max(sequence);
        }
        if let Some(sequence) = delta.proposals {
            state.template.proposals = state.template.proposals.max(sequence);
        }
        if let Some(sequence) = delta.transactions {
            state.template.transactions = state.template.transactions.max(sequence);
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
            relay_parents: Some(sequence),
            proposals: Some(sequence),
            transactions: Some(sequence),
            chain: Some(sequence),
        }
    }
}

impl AuthoritySourceVersionSnapshot {
    #[cfg(test)]
    pub(in crate::authority) fn relay_parents(self) -> ApplySequence {
        self.relay_parents
    }

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
                Self { template, ..self }
            }
            SourceImpact::Accepted => Self {
                relay_parents: self.relay_parents,
                template: PoolTemplateVersions {
                    proposals: sequence,
                    transactions: sequence,
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

        assert_eq!(versions.relay_parents(), ApplySequence(11));
        assert_eq!(
            versions.template(),
            PoolTemplateVersions {
                proposals: ApplySequence(11),
                transactions: ApplySequence(11),
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
    relay_parents: Option<ApplySequence>,
    proposals: Option<ApplySequence>,
    transactions: Option<ApplySequence>,
    chain: Option<ApplySequence>,
}

impl SourceVersionDelta {
    pub(super) fn is_template_selection_only(self) -> bool {
        self.relay_parents.is_none() && self.chain.is_none() && self.template_selection_changed()
    }

    pub(super) fn template_selection_changed(self) -> bool {
        self.proposals.is_some() || self.transactions.is_some()
    }

    pub(super) fn take_template_selection(
        &mut self,
    ) -> (Option<ApplySequence>, Option<ApplySequence>) {
        (self.proposals.take(), self.transactions.take())
    }

    fn is_empty(self) -> bool {
        self.relay_parents.is_none()
            && self.proposals.is_none()
            && self.transactions.is_none()
            && self.chain.is_none()
    }

    fn between(
        before: AuthoritySourceVersionSnapshot,
        after: AuthoritySourceVersionSnapshot,
    ) -> Self {
        Self {
            relay_parents: (before.relay_parents != after.relay_parents)
                .then_some(after.relay_parents),
            proposals: (before.template.proposals != after.template.proposals)
                .then_some(after.template.proposals),
            transactions: (before.template.transactions != after.template.transactions)
                .then_some(after.template.transactions),
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
            Self::Proposals => versions.proposals = sequence,
            Self::Transactions => versions.transactions = sequence,
            Self::Both => {
                versions.proposals = sequence;
                versions.transactions = sequence;
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
