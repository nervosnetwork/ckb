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
            .max()
            .unwrap_or(SourceImpact::None);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SourceImpact {
    None,
    Owners,
    Status,
    Accepted,
}

impl SourceImpact {
    fn for_replacement(before: Option<&OwnedTx>, after: Option<&OwnedTx>) -> Self {
        match (before, after) {
            (Some(OwnedTx::Accepted(before)), Some(OwnedTx::Accepted(after)))
                if before.record.identity == after.record.identity
                    && before.proof == after.proof =>
            {
                if before.proposal == after.proposal {
                    Self::None
                } else {
                    Self::Status
                }
            }
            (Some(OwnedTx::Accepted(_)), _) | (_, Some(OwnedTx::Accepted(_))) => Self::Accepted,
            (None, None) => Self::None,
            (Some(OwnedTx::PreAccepted(_)) | None, Some(OwnedTx::PreAccepted(_)) | None) => {
                Self::Owners
            }
        }
    }
}
