//! One verified candidate's admission attempts and optional history policy.

use super::{Batch, Entry, EntryCompleted, Error, Pool, Reject, Verified, membership};
use std::sync::Arc;

/// Commit membership and effects, or validate the same decision without mutation.
#[derive(Clone, Copy)]
pub(super) enum AdmissionMode {
    Commit,
    Preview,
}

// Verification/admission capacity is an ordinary local refusal. Errors from
// publication keep their service-failure meaning and do not pass this boundary.
pub(super) fn capacity_rejection(error: Error) -> Error {
    match error {
        Error::Full(reason) => Error::Rejected(Reject::Full(reason.to_string())),
        error => error,
    }
}

/// Replanning keeps the same candidate, predecessor and proof. Once optional
/// history loses capacity, later attempts omit it. A direct request completes
/// through `complete`; a worker applies the decision and leaves its committed
/// notices with the outbox.
pub(super) struct Admission<'a> {
    pool: &'a Pool,
    candidate: &'a Arc<Entry>,
    before: Option<&'a Arc<Entry>>,
    verified: &'a Verified,
    mode: AdmissionMode,
    retain_history: bool,
}

impl<'a> Admission<'a> {
    pub(super) fn new(
        pool: &'a Pool,
        candidate: &'a Arc<Entry>,
        before: Option<&'a Arc<Entry>>,
        verified: &'a Verified,
        mode: AdmissionMode,
    ) -> Self {
        Self {
            pool,
            candidate,
            before,
            verified,
            mode,
            retain_history: matches!(mode, AdmissionMode::Commit),
        }
    }

    /// Complete a direct request, including its required publication. Preview
    /// validates once without waiting for capacity or changing membership.
    pub(super) async fn complete(mut self) -> Result<EntryCompleted, Error> {
        let pool = self.pool;
        let mode = self.mode;
        let mut attempt = || {
            pool.open()?;
            self.apply()
        };
        let applied = match mode {
            AdmissionMode::Preview => attempt(),
            AdmissionMode::Commit => pool.commit_attempt(attempt).await,
        };
        let (batch, rejection) = applied.map_err(capacity_rejection)?;
        pool.published(batch).await?;
        match rejection {
            Some(reject) => Err(Error::Rejected(reject)),
            None => Ok(EntryCompleted {
                cycles: self.verified.cycles(),
                fee: self.verified.resolved().fee,
            }),
        }
    }

    /// A policy rejection is an applied decision too; its notice must complete
    /// before a direct submitter receives that rejection.
    pub(super) fn apply(&mut self) -> Result<(Option<Arc<Batch>>, Option<Reject>), Error> {
        let (plan, rejection) = membership::admission(
            &self.pool.store,
            self.candidate,
            self.before.cloned(),
            self.verified,
            &self.pool.config,
            self.retain_history,
        )?;
        let batch = match self.mode {
            AdmissionMode::Commit => self
                .pool
                .store
                .apply_admission(plan, &mut self.retain_history),
            AdmissionMode::Preview => self.pool.store.apply(plan.dry_run()),
        }?;
        Ok((batch, rejection))
    }

    /// Membership may become stale while the original verification premises
    /// still hold. Only then may a worker reuse this proof for another attempt.
    pub(super) fn check_verification(&self) -> Result<(), Error> {
        let resolved = self.verified.resolved();
        self.pool
            .store
            .read_selected(resolved.view, &resolved.reads, || ())
    }
}
