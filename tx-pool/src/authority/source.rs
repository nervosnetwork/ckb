//! Monotonic source versions compiled from authoritative owner transitions.
//!
//! These are not independent clocks and callers never publish template dirty
//! flags. Each field records the committed [`ApplySequence`] that last changed
//! the corresponding fact. The exhaustive before/after compiler below is the
//! only place that maps an owner transition to template work.

use super::state::{AcceptedStatus, ApplySequence, OwnedTx};

/// Source cut captured with accepted payloads for block-template work.
///
/// `proposals` and `transactions` are exact derived selection sources, so a
/// Gap/Proposed change does not cause unrelated proposal work and a
/// Pending/Gap change does not cause unrelated transaction work. General
/// accepted/status OCC versions stay in [`AuthoritySourceVersions`] and do
/// not widen this consumer receipt. Block-assembler configuration is immutable
/// after construction; chain-dependent policy is covered by `chain`, so there
/// is no producerless policy clock.
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
pub(super) struct AuthoritySourceVersions {
    owners: ApplySequence,
    accepted: ApplySequence,
    status: ApplySequence,
    template: PoolTemplateVersions,
}

impl AuthoritySourceVersions {
    pub(super) const fn initial() -> Self {
        Self {
            owners: ApplySequence(0),
            accepted: ApplySequence(0),
            status: ApplySequence(0),
            template: PoolTemplateVersions::initial(),
        }
    }

    pub(super) fn owners(self) -> ApplySequence {
        self.owners
    }

    pub(super) fn accepted(self) -> ApplySequence {
        self.accepted
    }

    pub(super) fn status(self) -> ApplySequence {
        self.status
    }

    pub(super) fn template(self) -> PoolTemplateVersions {
        self.template
    }

    pub(super) fn plan_replacements<'entry>(
        self,
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let impact = replacements
            .into_iter()
            .map(|(before, after)| SourceImpact::for_replacement(before, after))
            .fold(SourceImpact::None, SourceImpact::join);
        SourceVersionDelta {
            after: self.with_impact(impact, sequence),
        }
    }

    /// A chain transition changes the template chain source even when its
    /// accepted owner set is unchanged. Owner-derived selection sources still
    /// advance only according to the same exhaustive transition compiler.
    pub(super) fn plan_chain_replacements<'entry>(
        self,
        replacements: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        sequence: ApplySequence,
    ) -> SourceVersionDelta {
        let mut delta = self.plan_replacements(replacements, sequence);
        delta.after.template.chain = sequence;
        delta
    }

    pub(super) fn apply(&mut self, delta: SourceVersionDelta) {
        *self = delta.after;
    }

    pub(super) fn plan_generation_replacement(self, sequence: ApplySequence) -> SourceVersionDelta {
        SourceVersionDelta {
            after: Self {
                owners: sequence,
                accepted: sequence,
                status: sequence,
                template: PoolTemplateVersions {
                    proposals: sequence,
                    transactions: sequence,
                    chain: sequence,
                },
            },
        }
    }

    fn with_impact(self, impact: SourceImpact, sequence: ApplySequence) -> Self {
        match impact {
            SourceImpact::None => self,
            SourceImpact::Owners => Self {
                owners: sequence,
                ..self
            },
            SourceImpact::Status(selection) => {
                let mut template = self.template;
                selection.advance(&mut template, sequence);
                Self {
                    owners: sequence,
                    status: sequence,
                    template,
                    ..self
                }
            }
            SourceImpact::Accepted => Self {
                owners: sequence,
                accepted: sequence,
                status: sequence,
                template: PoolTemplateVersions {
                    proposals: sequence,
                    transactions: sequence,
                    ..self.template
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceVersionDelta {
    after: AuthoritySourceVersions,
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
    Owners,
    Status(TemplateSelectionImpact),
    Accepted,
}

impl SourceImpact {
    fn join(self, incoming: Self) -> Self {
        match (self, incoming) {
            (Self::Accepted, _) | (_, Self::Accepted) => Self::Accepted,
            (Self::Status(left), Self::Status(right)) => Self::Status(left.join(right)),
            (Self::Status(impact), Self::None | Self::Owners)
            | (Self::None | Self::Owners, Self::Status(impact)) => Self::Status(impact),
            (Self::Owners, Self::None | Self::Owners) | (Self::None, Self::Owners) => Self::Owners,
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
            | (None, Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_))) => {
                Self::Owners
            }
        }
    }
}

#[cfg(test)]
pub(super) fn replacement_changes_accepted_source_for_foundation(
    before: &OwnedTx,
    after: &OwnedTx,
) -> bool {
    matches!(
        SourceImpact::for_replacement(Some(before), Some(after)),
        SourceImpact::Accepted
    )
}
