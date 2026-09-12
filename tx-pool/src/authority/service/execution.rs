//! Worker execution and complete job settlement for the single Pool.

use super::*;

impl Pool {
    pub(super) async fn requeue(&self, job: &mut Job) -> Result<(), Error> {
        self.commit(|| {
            let (view, _) = self.store.snapshot();
            let mut plan = Plan::new(view, ingress::class(job.entry.source), Default::default());
            plan.edit(
                Some(Arc::clone(&job.entry)),
                Some(job.entry.with_phase(Phase::Resolve)),
                None,
            )?;
            Ok(plan)
        })
        .await
        .or_else(|error| {
            if matches!(error, Error::Stale) {
                Ok(None)
            } else {
                Err(error)
            }
        })?;
        job.complete();
        Ok(())
    }
    pub(super) async fn reject_job(
        &self,
        job: &mut Job,
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
            Ok(_) => {
                // The committed outbox owns publication through drain or fault.
                // Background work can return its worker and active reservation.
                job.complete();
                Ok(())
            }
            Err(Error::Stale) => self.requeue(job).await,
            Err(error) => Err(error),
        }
    }
    pub(super) async fn resolve_job(
        &self,
        job: &mut Job,
        cpu: OwnedSemaphorePermit,
    ) -> Result<(), Error> {
        let resolution = self.run_compute(&cpu, || {
            jobs::resolve(&self.store, &job.entry, &self.config)
        });
        drop(cpu);
        let resolution = match resolution {
            Ok(resolution) => resolution,
            Err(Error::Stale) => return self.requeue(job).await,
            Err(Error::Full(reason)) => {
                return self
                    .reject_job(job, Reject::Full(reason.to_string()), ReadSet::default())
                    .await;
            }
            Err(error) => return Err(error),
        };
        let result = self
            .commit(|| ingress::resolution(&self.store, job.view, &job.entry, &resolution))
            .await;
        match result {
            Ok(_) => {
                job.complete();
                Ok(())
            }
            Err(Error::Stale) => self.requeue(job).await,
            Err(Error::Full(reason)) if !matches!(resolution, Resolution::Rejected(..)) => {
                self.reject_job(job, Reject::Full(reason.to_string()), ReadSet::default())
                    .await
            }
            Err(error) => Err(error),
        }
    }
    pub(super) async fn accept_job(&self, job: &mut Job, verified: &Verified) -> Result<(), Error> {
        let mut retain_history = true;
        loop {
            if !job.current()? {
                job.complete();
                return Ok(());
            }
            if self.store.is_stopped() {
                return self.requeue(job).await;
            }
            let result = self
                .commit_attempt(|| {
                    membership::admission(
                        &self.store,
                        &job.entry,
                        Some(Arc::clone(&job.entry)),
                        verified,
                        &self.config,
                        retain_history,
                    )
                    .and_then(|(plan, _)| self.store.apply_admission(plan, &mut retain_history))
                })
                .await;
            match result {
                Ok(_) => {
                    job.complete();
                    return Ok(());
                }
                Err(Error::Stale) => {
                    match self.store.read_selected(
                        verified.resolved().view,
                        &verified.resolved().reads,
                        || (),
                    ) {
                        Ok(()) => tokio::task::yield_now().await,
                        Err(Error::Stale) => return self.requeue(job).await,
                        Err(error) => return Err(error),
                    }
                }
                Err(Error::Full(reason)) => {
                    return self
                        .reject_job(job, Reject::Full(reason.to_string()), ReadSet::default())
                        .await;
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
                let small_only = primary_stage == WorkStage::Verify
                    && index == 0
                    && self.store.budget.limits.workers > 1;
                let selected = match primary_stage {
                    WorkStage::Resolve => self.store.pop(WorkStage::Resolve, false),
                    WorkStage::Verify => {
                        self.store
                            .pop(WorkStage::Verify, small_only)
                            .and_then(|selected| match selected {
                                Some(_) => Ok(selected),
                                None => self.store.pop(WorkStage::Resolve, small_only),
                            })
                    }
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
                    self.mode,
                )
                .await;
                drop(cpu);
                if self.store.is_stopped() {
                    self.requeue(&mut job).await?;
                    return Ok(());
                }
                match verified {
                    Ok(verified) => self.accept_job(&mut job, &verified).await?,
                    Err(Error::Stale) => self.requeue(&mut job).await?,
                    Err(Error::Rejected(reject)) => {
                        self.reject_job(&mut job, reject, resolved.reads.clone())
                            .await?
                    }
                    Err(Error::Full(reason)) => {
                        self.reject_job(
                            &mut job,
                            Reject::Full(reason.to_string()),
                            ReadSet::default(),
                        )
                        .await?
                    }
                    Err(error) => return Err(error),
                }
            } else {
                self.resolve_job(&mut job, cpu).await?;
            }
        }
    }
}
