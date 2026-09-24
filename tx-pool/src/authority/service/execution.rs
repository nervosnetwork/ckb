//! Worker execution and complete job settlement for the single Pool.

use super::admission::{Admission, AdmissionMode};
use super::*;

impl Pool {
    pub(super) async fn requeue(&self, job: &Job) -> Result<(), Error> {
        // A stale view does not retire the selected owner. Its queue item was
        // consumed, so settlement must commit a successor or observe retirement.
        while job.current() {
            let result = self
                .commit(|| {
                    let (view, _) = self.store.snapshot();
                    let mut plan =
                        Plan::new(view, ingress::class(job.entry.source), Default::default());
                    plan.edit(
                        Some(Arc::clone(&job.entry)),
                        Some(job.entry.with_phase(Phase::Resolve)),
                        None,
                    )?;
                    Ok(plan)
                })
                .await;
            match result {
                Ok(_) => return Ok(()),
                Err(Error::Stale) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub(super) async fn reject_job(
        &self,
        job: &Job,
        reject: Reject,
        reads: ReadSet,
    ) -> Result<(), Error> {
        let result = self
            .commit(|| {
                ingress::rejection(
                    &self.store,
                    Plan::new(job.view, ingress::class(job.entry.source), reads.clone()),
                    Some(Arc::clone(&job.entry)),
                    &job.entry.hash(),
                    job.entry.source,
                    reject.clone(),
                )
            })
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(Error::Stale) => self.requeue(job).await,
            Err(error) => Err(error),
        }
    }
    // Capacity refusal is pool policy, independent of resolved transaction
    // facts. Rejection still settles the selected owner, including stale requeue.
    async fn reject_capacity(&self, job: &Job, reason: FullReason) -> Result<(), Error> {
        self.reject_job(job, Reject::Full(reason.to_string()), ReadSet::default())
            .await
    }
    pub(super) async fn resolve_job(&self, job: &Job, cpu: ComputePermit) -> Result<(), Error> {
        let resolution = cpu.run(|| jobs::resolve(&self.store, &job.entry, &self.config));
        drop(cpu);
        let resolution = match resolution {
            Ok(resolution) => resolution,
            Err(Error::Stale) => return self.requeue(job).await,
            Err(Error::Full(reason)) => {
                return self.reject_capacity(job, reason).await;
            }
            Err(error) => return Err(error),
        };
        let result = self
            .commit(|| ingress::resolution(&self.store, job.view, &job.entry, &resolution))
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(Error::Stale) => self.requeue(job).await,
            Err(Error::Full(reason)) if !matches!(resolution, Resolution::Rejected(..)) => {
                self.reject_capacity(job, reason).await
            }
            Err(error) => Err(error),
        }
    }
    pub(super) async fn accept_job(&self, job: &Job, verified: &Verified) -> Result<(), Error> {
        let mut admission = Admission::new(
            self,
            &job.entry,
            Some(&job.entry),
            verified,
            AdmissionMode::Commit,
        );
        loop {
            if !job.current() {
                return Ok(());
            }
            if self.store.is_stopped() {
                return self.requeue(job).await;
            }
            let result = self.commit_attempt(|| admission.apply()).await;
            match result {
                Ok(_) => return Ok(()),
                Err(Error::Stale) => match admission.check_verification() {
                    Ok(()) => tokio::task::yield_now().await,
                    Err(Error::Stale) => return self.requeue(job).await,
                    Err(error) => return Err(error),
                },
                Err(Error::Full(reason)) => {
                    return self.reject_capacity(job, reason).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub(super) async fn worker(
        self: Arc<Self>,
        primary_stage: WorkStage,
        index: usize,
    ) -> Result<(), Error> {
        let mut commands = self.commands.clone();
        loop {
            // Queue and capacity subscriptions belong only to selection. Once
            // a job is owned, unrelated commits must not repoll its VM future.
            let (cpu, mut job) = {
                let work = self.store.work.notified();
                let memory = self.store.budget.changed.notified();
                tokio::pin!(work, memory);
                // Queue repair also uses notify_one, which needs registration.
                work.as_mut().enable();
                if self.store.is_faulted() {
                    return Err(Error::Fault("worker generation"));
                }
                if self.store.is_stopped() {
                    return Ok(());
                }
                let cpu = match self.compute().await {
                    Ok(cpu) => cpu,
                    Err(Error::Closed) => return Ok(()),
                    Err(error) => return Err(error),
                };
                let selection = if primary_stage == WorkStage::Verify
                    && index == 0
                    && self.store.budget.limits.workers > 1
                {
                    WorkSelection::SmallOnly
                } else {
                    WorkSelection::Any
                };
                let selected = match primary_stage {
                    WorkStage::Resolve => self.store.pop(WorkStage::Resolve, WorkSelection::Any),
                    WorkStage::Verify => match self.store.pop(WorkStage::Verify, selection) {
                        Ok(None) => self.store.pop(WorkStage::Resolve, selection),
                        selected => selected,
                    },
                };
                let job = match selected {
                    Ok(Some(job)) => job,
                    Ok(None) | Err(Error::Full(_)) => {
                        drop(cpu);
                        tokio::select! {
                            _ = &mut work => {},
                            _ = &mut memory => {},
                            _ = self.stopped.cancelled() => return Ok(())
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                (cpu, job)
            };
            if let Phase::Verify(resolved) = &job.entry.phase {
                let resolved = Arc::clone(resolved);
                let verified = jobs::verify(
                    &self.store,
                    &job.entry,
                    Arc::clone(&resolved),
                    &self.config,
                    &self.cache,
                    &mut commands,
                    &cpu,
                )
                .await;
                drop(cpu);
                if self.store.is_stopped() {
                    self.requeue(&job).await?;
                    job.mark_handled();
                    return Ok(());
                }
                match verified {
                    Ok(verified) => self.accept_job(&job, &verified).await?,
                    Err(Error::Stale) => self.requeue(&job).await?,
                    Err(Error::Rejected(reject)) => {
                        self.reject_job(&job, reject, resolved.reads.clone())
                            .await?
                    }
                    Err(Error::Full(reason)) => self.reject_capacity(&job, reason).await?,
                    Err(error) => return Err(error),
                }
            } else {
                self.resolve_job(&job, cpu).await?;
            }
            // Only the worker owns completion. Successful settlement either
            // committed a successor or observed one; the outbox owns notices.
            // Keep the active reservation until this frame and its results drop.
            job.mark_handled();
        }
    }
}
