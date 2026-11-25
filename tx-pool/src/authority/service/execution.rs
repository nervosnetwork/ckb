//! Worker execution and complete job settlement for the single Pool.

use super::{membership::AdmissionOptions, *};

impl Pool {
    pub(super) async fn requeue(&self, job: &Job) -> Result<(), Error> {
        let hash = job.entry.hash_bytes();
        while job.current() {
            let result = self
                .commit_attempt(|| {
                    let (_, batch) = self.store.commit_with(|tx| {
                        if !tx
                            .state
                            .owner(&hash)
                            .is_some_and(|current| Arc::ptr_eq(current, &job.entry))
                        {
                            return Err(Error::Stale);
                        }
                        let after = job.entry.try_with_phase(Phase::Resolve)?;
                        tx.commit_single(
                            hash,
                            Some(Arc::clone(&job.entry)),
                            Some(after),
                            Vec::new(),
                            Class::Trusted,
                        )
                    })?;
                    Ok(batch)
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
        premises: Premises,
    ) -> Result<(), Error> {
        let result = self
            .commit_attempt(|| {
                ingress::commit_rejection(
                    &self.store,
                    job.view,
                    &premises,
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

    async fn reject_capacity(&self, job: &Job, reason: FullReason) -> Result<(), Error> {
        self.reject_job(job, Reject::Full(reason.to_string()), Premises::default())
            .await
    }

    async fn settle_resolution(&self, job: &Job, resolution: Resolution) -> Result<(), Error> {
        let result = self
            .commit_attempt(|| {
                ingress::commit_resolution(&self.store, job.view, &job.entry, &resolution)
            })
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

    async fn settle_verification(
        &self,
        job: &Job,
        resolved: &super::super::model::Resolved,
        verified: Result<Verified, Error>,
    ) -> Result<(), Error> {
        if self.store.is_stopped() {
            return self.requeue(job).await;
        }
        match verified {
            Ok(verified) => self.accept_job(job, &verified).await,
            Err(Error::Stale) => self.requeue(job).await,
            Err(Error::Rejected(reject)) => {
                self.reject_job(job, reject, resolved.premises.clone())
                    .await
            }
            Err(Error::Full(reason)) => self.reject_capacity(job, reason).await,
            Err(error) => Err(error),
        }
    }

    pub(super) async fn resolve_job(&self, job: &Job, cpu: ComputePermit) -> Result<(), Error> {
        let store = Arc::clone(&self.store);
        let entry = Arc::clone(&job.entry);
        let config = Arc::clone(&self.config);
        let resolution = self
            .compute
            .run_permitted(move || jobs::resolve(&store, &entry, &config))
            .await?;
        drop(cpu);
        match resolution {
            Ok(resolution) => self.settle_resolution(job, resolution).await,
            Err(Error::Stale) => self.requeue(job).await,
            Err(Error::Full(reason)) => self.reject_capacity(job, reason).await,
            Err(error) => Err(error),
        }
    }

    pub(super) async fn accept_job(&self, job: &Job, verified: &Verified) -> Result<(), Error> {
        if !job.current() {
            return Ok(());
        }
        let result = self
            .commit_attempt(|| {
                self.open()?;
                membership::commit_admission(
                    &self.store,
                    &job.entry,
                    Some(Arc::clone(&job.entry)),
                    verified,
                    &self.config,
                    AdmissionOptions::commit(),
                )
            })
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(Error::Stale) => self.requeue(job).await,
            Err(Error::Closed) if self.store.is_stopped() => self.requeue(job).await,
            Err(Error::Full(reason)) => self.reject_capacity(job, reason).await,
            Err(error) => Err(error),
        }
    }

    /// Fused `Resolve -> Verify -> Accept` fast path for `Verify` workers
    /// whenever `queues.verify` is empty, skipping the intermediate `Phase::Verify`
    /// write lock and queue round-trip.
    async fn resolve_and_verify_fused(
        &self,
        job: &Job,
        cpu: ComputePermit,
        commands: &mut watch::Receiver<ChunkCommand>,
    ) -> Result<(), Error> {
        let store = Arc::clone(&self.store);
        let entry = Arc::clone(&job.entry);
        let config = Arc::clone(&self.config);
        let view = job.view;
        let (snapshot, resolution) = match self
            .compute
            .run_permitted(move || jobs::resolve_for_view(&store, &entry, &config, view))
            .await?
        {
            Ok(pair) => pair,
            Err(Error::Stale) => {
                drop(cpu);
                return self.requeue(job).await;
            }
            Err(Error::Full(reason)) => {
                drop(cpu);
                return self.reject_capacity(job, reason).await;
            }
            Err(error) => {
                drop(cpu);
                return Err(error);
            }
        };
        let Resolution::Ready(resolved) = resolution else {
            drop(cpu);
            return self.settle_resolution(job, resolution).await;
        };
        if self.store.is_stopped() {
            drop(cpu);
            return self.requeue(job).await;
        }
        let verified = jobs::verify_with_snapshot(
            #[cfg(any(test, feature = "internal"))]
            &self.store,
            snapshot,
            &job.entry,
            Arc::clone(&resolved),
            &self.config,
            &self.cache,
            commands,
            &cpu,
        )
        .await;
        drop(cpu);
        self.settle_verification(job, &resolved, verified).await
    }

    pub(super) async fn worker(
        self: Arc<Self>,
        primary_stage: WorkStage,
        index: usize,
    ) -> Result<(), Error> {
        let mut commands = self.commands.clone();
        loop {
            let (cpu, mut job) = {
                let work = self.store.work.notified();
                let memory = self.store.budget.active_changed.notified();
                tokio::pin!(work, memory);
                if self.store.is_faulted() {
                    return Err(Error::Fault("worker generation"));
                }
                if self.store.is_stopped() {
                    return Ok(());
                }
                if !self.store.has_queued_work() {
                    tokio::select! {
                        _ = &mut work => {},
                        _ = &mut memory => {},
                        _ = self.stopped.cancelled() => return Ok(())
                    }
                    continue;
                }
                let cpu = match self.compute_background().await {
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
                self.settle_verification(&job, &resolved, verified).await?;
                if self.store.is_stopped() {
                    job.mark_handled();
                    return Ok(());
                }
            } else if primary_stage == WorkStage::Verify && !self.store.has_verify_work() {
                self.resolve_and_verify_fused(&job, cpu, &mut commands)
                    .await?;
            } else {
                self.resolve_job(&job, cpu).await?;
            }
            job.mark_handled();
        }
    }
}
