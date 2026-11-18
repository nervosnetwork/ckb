//! One verified candidate's admission attempts and optional history policy.

use super::{Batch, Entry, EntryCompleted, Error, Plan, Pool, Reject, Verified, membership};
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

/// The original facts for one verified candidate. Only its membership plan
/// may change when another admission wins the commit.
pub(super) struct Admission<'a> {
    pub(super) pool: &'a Pool,
    pub(super) candidate: &'a Arc<Entry>,
    pub(super) before: Option<&'a Arc<Entry>>,
    pub(super) verified: &'a Verified,
}

impl Admission<'_> {
    /// Complete a direct request, including its required publication. Preview
    /// validates the plan without waiting for capacity or changing membership.
    pub(super) async fn complete(&self, mode: AdmissionMode) -> Result<EntryCompleted, Error> {
        let pool = self.pool;
        let rejection = match mode {
            AdmissionMode::Preview => self.preview().map_err(capacity_rejection)?,
            AdmissionMode::Commit => {
                let (batch, rejection) = self
                    .commit_while(|| pool.computation_ready().is_ok())
                    .await
                    .map_err(capacity_rejection)?;
                pool.published(batch).await?;
                rejection
            }
        };
        match rejection {
            Some(reject) => Err(Error::Rejected(reject)),
            None => Ok(EntryCompleted {
                cycles: self.verified.cycles(),
                fee: self.verified.resolved().fee,
            }),
        }
    }

    /// A membership conflict can change the victims without changing the
    /// verified transaction. Replan with the same proof and history decision
    /// while its original view, inputs and candidate predecessor still hold.
    pub(super) async fn commit_while(
        &self,
        can_replan: impl Fn() -> bool,
    ) -> Result<(Option<Arc<Batch>>, Option<Reject>), Error> {
        let pool = self.pool;
        // Once optional history loses capacity, every remaining plan omits it.
        let mut retain_history = true;
        let mut attempt = || {
            let (plan, rejection) = self.plan(retain_history)?;
            let batch = pool.store.apply_admission(plan, &mut retain_history)?;
            Ok((batch, rejection))
        };
        loop {
            match pool.commit_attempt(&mut attempt).await {
                Err(Error::Stale) => {}
                result => return result,
            }
            tokio::task::yield_now().await;
            if !can_replan() {
                return Err(Error::Stale);
            }
            let resolved = self.verified.resolved();
            let reads = resolved
                .reads_with_predecessor(self.candidate.transaction.hash_ref(), self.before)?;
            pool.store.read_selected(resolved.view, &reads, || ())?;
        }
    }

    fn preview(&self) -> Result<Option<Reject>, Error> {
        let (plan, rejection) = self.plan(false)?;
        self.pool.store.apply(plan.dry_run())?;
        Ok(rejection)
    }

    fn plan(&self, retain_history: bool) -> Result<(Plan, Option<Reject>), Error> {
        self.pool.open()?;
        membership::admission(
            &self.pool.store,
            self.candidate,
            self.before.cloned(),
            self.verified,
            &self.pool.config,
            retain_history,
        )
    }
}
