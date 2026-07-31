//! Monotonic source versions compiled from authoritative owner transitions.
//!
//! These are not independent counters. Each component records the committed
//! `ApplySequence` that last changed it, so ChainPlan OCC can ignore unrelated
//! pre-acceptance compute while detecting every accepted/status mutation.

use super::state::{ApplySequence, OwnedTx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuthoritySourceVersions {
    pub(super) owners: ApplySequence,
    pub(super) accepted: ApplySequence,
    pub(super) status: ApplySequence,
}

impl AuthoritySourceVersions {
    pub(super) const fn initial() -> Self {
        Self {
            owners: ApplySequence(0),
            accepted: ApplySequence(0),
            status: ApplySequence(0),
        }
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
        let after = match impact {
            SourceImpact::None => self,
            SourceImpact::Owners => Self {
                owners: sequence,
                ..self
            },
            SourceImpact::Status => Self {
                owners: sequence,
                status: sequence,
                ..self
            },
            SourceImpact::Accepted => Self {
                owners: sequence,
                accepted: sequence,
                status: sequence,
            },
        };
        SourceVersionDelta { after }
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
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceVersionDelta {
    after: AuthoritySourceVersions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceImpact {
    None,
    Owners,
    Status,
    Accepted,
}

impl SourceImpact {
    fn join(self, incoming: Self) -> Self {
        match self {
            Self::None => match incoming {
                Self::None => Self::None,
                Self::Owners => Self::Owners,
                Self::Status => Self::Status,
                Self::Accepted => Self::Accepted,
            },
            Self::Owners => match incoming {
                Self::None | Self::Owners => Self::Owners,
                Self::Status => Self::Status,
                Self::Accepted => Self::Accepted,
            },
            Self::Status => match incoming {
                Self::None | Self::Owners | Self::Status => Self::Status,
                Self::Accepted => Self::Accepted,
            },
            Self::Accepted => match incoming {
                Self::None | Self::Owners | Self::Status | Self::Accepted => Self::Accepted,
            },
        }
    }

    fn for_replacement(before: Option<&OwnedTx>, after: Option<&OwnedTx>) -> Self {
        match (before, after) {
            (Some(OwnedTx::Accepted(before)), Some(OwnedTx::Accepted(after))) => {
                // EntryVersion is OCC machinery, not an accepted fact. Every
                // other immutable record field participates in the source
                // cut so a future metadata transition cannot masquerade as a
                // status-only update.
                if before.record.identity != after.record.identity
                    || before.record.arrival != after.record.arrival
                    || before.provenance != after.provenance
                    || before.proof != after.proof
                {
                    Self::Accepted
                } else if before.proposal != after.proposal {
                    Self::Status
                } else {
                    Self::None
                }
            }
            (Some(OwnedTx::Accepted(_)), Some(OwnedTx::PreAccepted(_)) | None)
            | (Some(OwnedTx::PreAccepted(_)) | None, Some(OwnedTx::Accepted(_))) => Self::Accepted,
            (None, None) => Self::None,
            (Some(OwnedTx::PreAccepted(_)), Some(OwnedTx::PreAccepted(_)) | None)
            | (None, Some(OwnedTx::PreAccepted(_))) => Self::Owners,
        }
    }
}
