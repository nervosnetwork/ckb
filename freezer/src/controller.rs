use crate::{Freezer, internal_error};
use ckb_error::Error;
use ckb_types::{packed, prelude::*};
use ckb_util::Mutex;
use std::{ops::Range, sync::Arc};
use tokio::{
    runtime::Handle,
    sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
};

/// Bounds for accepted background I/O. Synchronous reads use the direct path.
#[derive(Clone, Debug)]
pub struct FreezerServiceConfig {
    /// Maximum queued write batches, excluding the running batch.
    pub write_queue: usize,
    /// Maximum concurrently running asynchronous read batches.
    pub read_workers: usize,
    /// Maximum records in either kind of batch.
    pub batch_records: usize,
    /// Combined payload-buffer budget for queued writes and active reads/writes.
    pub io_bytes: u32,
}

impl Default for FreezerServiceConfig {
    fn default() -> Self {
        Self {
            write_queue: 32,
            read_workers: 16,
            batch_records: 512,
            io_bytes: 128 << 20,
        }
    }
}

/// A synchronous reader and bounded asynchronous I/O admission boundary.
///
/// Cancellation before admission performs no I/O. Once accepted, an append
/// commits even if its caller drops the response. Reads already started also
/// finish. `shutdown` closes admission and waits for that work and the writer's
/// final sync; cancelling the shutdown future is safe, and it can be awaited again.
/// The supplied runtime must remain running until shutdown completes.
#[derive(Clone)]
pub struct FreezerController(Arc<Inner>);

/// An approximate view of background I/O admission and occupancy.
#[derive(Debug)]
pub struct FreezerServiceStatus {
    /// Whether admission of new background I/O requests is closed.
    pub shutting_down: bool,
    /// Accepted read/write batches that have not finished.
    pub accepted_requests: usize,
    /// Includes queue slots reserved by callers before enqueueing.
    pub reserved_write_slots: usize,
    /// Read-worker permits currently reserved or in use.
    pub reserved_read_slots: usize,
    /// Includes byte permits reserved by callers waiting for a worker or queue.
    pub reserved_bytes: usize,
}

struct Inner {
    freezer: Freezer,
    runtime: Handle,
    config: FreezerServiceConfig,
    writes: mpsc::Sender<Append>,
    read_slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    activity: Arc<Activity>,
    stop: watch::Sender<bool>,
    writer_done: watch::Receiver<Option<Result<(), String>>>,
}

#[derive(Default)]
struct Activity {
    // Admission and shutdown linearize on this lock. No I/O runs under it.
    state: Mutex<Admission>,
    drained: Notify,
}

#[derive(Default)]
struct Admission {
    shutting_down: bool,
    requests: usize,
}

impl Activity {
    fn admit(self: &Arc<Self>, bytes: OwnedSemaphorePermit) -> Result<Request, Error> {
        let mut state = self.state.lock();
        if state.shutting_down {
            return Err(internal_error("freezer service is shut down"));
        }
        state.requests += 1;
        Ok(Request {
            activity: Arc::clone(self),
            _bytes: bytes,
        })
    }
}

struct Request {
    activity: Arc<Activity>,
    _bytes: OwnedSemaphorePermit,
}

impl Drop for Request {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock();
        state.requests -= 1;
        if state.requests == 0 {
            self.activity.drained.notify_waiters();
        }
    }
}

struct Append {
    blocks: Vec<packed::Block>,
    response: oneshot::Sender<Result<Range<u64>, Error>>,
    _request: Request,
}

impl FreezerController {
    /// Start the serial writer on an existing Tokio runtime.
    pub fn start(
        freezer: Freezer,
        runtime: Handle,
        config: FreezerServiceConfig,
    ) -> Result<Self, Error> {
        if config.write_queue == 0
            || config.read_workers == 0
            || config.batch_records == 0
            || config.io_bytes == 0
            || config.read_workers > Semaphore::MAX_PERMITS
            || config.write_queue > Semaphore::MAX_PERMITS
            || config.io_bytes as usize > Semaphore::MAX_PERMITS
        {
            return Err(internal_error("invalid freezer service limits"));
        }
        let (writes, receiver) = mpsc::channel(config.write_queue);
        let (stop, stopping) = watch::channel(false);
        let (done, writer_done) = watch::channel(None);
        let writer = freezer.clone();
        runtime.spawn(async move {
            let result = write_loop(writer, receiver, stopping)
                .await
                .map_err(|error| error.to_string());
            done.send_replace(Some(result));
        });
        Ok(Self(Arc::new(Inner {
            freezer,
            runtime,
            read_slots: Arc::new(Semaphore::new(config.read_workers)),
            bytes: Arc::new(Semaphore::new(config.io_bytes as usize)),
            config,
            writes,
            activity: Arc::new(Activity::default()),
            stop,
            writer_done,
        })))
    }

    /// Read one committed record directly, without executor or queue overhead.
    /// Reads remain available after background I/O is shut down.
    pub fn retrieve(&self, record: u64) -> Result<Option<Vec<u8>>, Error> {
        self.0.freezer.retrieve(record)
    }

    /// The next record number after the committed prefix.
    pub fn number(&self) -> u64 {
        self.0.freezer.number()
    }

    /// Sample occupancy without entering the writer or waiting for disk I/O.
    pub fn status(&self) -> FreezerServiceStatus {
        let state = self.0.activity.state.lock();
        FreezerServiceStatus {
            shutting_down: state.shutting_down,
            accepted_requests: state.requests,
            reserved_write_slots: self.0.config.write_queue - self.0.writes.capacity(),
            reserved_read_slots: self.0.config.read_workers - self.0.read_slots.available_permits(),
            reserved_bytes: self.0.config.io_bytes as usize - self.0.bytes.available_permits(),
        }
    }

    /// Queue a durable append. The returned range identifies the input records.
    pub async fn append(&self, blocks: Vec<packed::Block>) -> Result<Range<u64>, Error> {
        self.check_count(blocks.len())?;
        let mut bytes = 0usize;
        let mut compressed = 0;
        for block in &blocks {
            let size = block.as_slice().len();
            bytes = bytes
                .checked_add(size)
                .ok_or_else(|| internal_error("append byte count overflow"))?;
            compressed =
                compressed.max(crate::payload::encoded_size_bound(size).map_err(internal_error)?);
        }
        let bytes = bytes
            .checked_add(compressed)
            .ok_or_else(|| internal_error("append byte count overflow"))?;
        let budget = self.reserve_bytes(bytes).await?;
        let slot = self.0.writes.reserve().await.map_err(internal_error)?;
        let (response, received) = oneshot::channel();
        let request = self.0.activity.admit(budget)?;
        slot.send(Append {
            blocks,
            response,
            _request: request,
        });
        received.await.map_err(internal_error)?
    }

    /// Read a bounded batch. `max_bytes` includes decoded results and the largest
    /// concurrent compressed buffer. Input/result metadata is bounded by record count.
    pub async fn retrieve_many(
        &self,
        records: Vec<u64>,
        max_bytes: u32,
    ) -> Result<Vec<Option<Vec<u8>>>, Error> {
        self.check_count(records.len())?;
        let budget = self.reserve_bytes(max_bytes as usize).await?;
        let slot = Arc::clone(&self.0.read_slots)
            .acquire_owned()
            .await
            .map_err(internal_error)?;
        let request = self.0.activity.admit(budget)?;
        let reader = Arc::clone(&self.0.freezer.reader);
        self.0
            .runtime
            .spawn_blocking(move || {
                let (_request, _slot) = (request, slot);
                let mut remaining = max_bytes as usize;
                records
                    .into_iter()
                    .map(|record| {
                        let data = reader
                            .retrieve_limited(record, remaining)
                            .map_err(internal_error)?;
                        remaining -= data.as_ref().map_or(0, Vec::len);
                        Ok(data)
                    })
                    .collect()
            })
            .await
            .map_err(internal_error)?
    }

    /// Close admission, drain accepted I/O, sync the writer, and report its result.
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.0.activity.state.lock().shutting_down = true;
        self.0.bytes.close();
        self.0.read_slots.close();
        self.0.stop.send_replace(true);
        loop {
            // Register before checking the count so completion cannot be missed.
            let drained = self.0.activity.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if self.0.activity.state.lock().requests == 0 {
                break;
            }
            drained.await;
        }
        let mut done = self.0.writer_done.clone();
        loop {
            if let Some(result) = done.borrow().clone() {
                return result.map_err(internal_error);
            }
            done.changed().await.map_err(internal_error)?;
        }
    }

    fn check_count(&self, count: usize) -> Result<(), Error> {
        if count == 0 || count > self.0.config.batch_records {
            return Err(internal_error("invalid freezer batch record count"));
        }
        Ok(())
    }

    async fn reserve_bytes(&self, bytes: usize) -> Result<OwnedSemaphorePermit, Error> {
        if bytes == 0 || bytes > self.0.config.io_bytes as usize {
            return Err(internal_error("freezer request exceeds I/O byte budget"));
        }
        Arc::clone(&self.0.bytes)
            .acquire_many_owned(bytes as u32)
            .await
            .map_err(internal_error)
    }
}

async fn write_loop(
    freezer: Freezer,
    mut receiver: mpsc::Receiver<Append>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let mut closing = false;
    loop {
        let request = tokio::select! {
            _ = stop.changed(), if !closing => {
                closing = true;
                receiver.close();
                continue;
            }
            request = receiver.recv() => request,
        };
        let Some(request) = request else { break };
        let writer = freezer.clone();
        tokio::task::spawn_blocking(move || {
            let Append {
                blocks,
                response,
                _request,
            } = request;
            let result = writer.append_blocks(&blocks);
            let _ = response.send(result);
        })
        .await
        .map_err(internal_error)?;
    }
    tokio::task::spawn_blocking(move || freezer.sync())
        .await
        .map_err(internal_error)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::core::{BlockBuilder, EpochNumberWithFraction};
    use std::{sync::mpsc as sync_mpsc, time::Duration};

    fn block(number: u64) -> packed::Block {
        BlockBuilder::default()
            .number(number)
            .epoch(EpochNumberWithFraction::new(1, 0, 100))
            .build()
            .data()
    }

    #[test]
    fn chunked_append_accounts_for_table_and_compression_capacity() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let block = BlockBuilder::default()
            .transaction(
                ckb_types::core::TransactionBuilder::default()
                    .witness(packed::Bytes::from(vec![9; 64 * 1024]))
                    .build(),
            )
            .build()
            .data();
        let size = block.as_slice().len();
        let required = size + crate::payload::encoded_size_bound(size).unwrap();
        for budget in [required - 1, required] {
            let directory = tempfile::tempdir().unwrap();
            let controller = FreezerController::start(
                Freezer::open_in(directory.path()).unwrap(),
                runtime.handle().clone(),
                FreezerServiceConfig {
                    io_bytes: budget as u32,
                    ..Default::default()
                },
            )
            .unwrap();
            runtime.block_on(async {
                let result = controller.append(vec![block.clone()]).await;
                assert_eq!(result.is_ok(), budget == required);
                assert_eq!(controller.number(), if budget == required { 2 } else { 1 });
                controller.shutdown().await.unwrap();
                assert_eq!(controller.status().reserved_bytes, 0);
                if budget == required {
                    assert_eq!(controller.retrieve(1).unwrap().unwrap(), block.as_slice());
                }
            });
        }
    }

    #[test]
    fn controller_limits_ordering_and_both_runtime_flavors() {
        for threaded in [false, true] {
            let mut builder = if threaded {
                tokio::runtime::Builder::new_multi_thread()
            } else {
                tokio::runtime::Builder::new_current_thread()
            };
            let runtime = builder.worker_threads(2).build().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let freezer = Freezer::open_in(directory.path()).unwrap();
            let config = FreezerServiceConfig {
                write_queue: 2,
                read_workers: 2,
                batch_records: 8,
                io_bytes: 8192,
            };
            let controller =
                FreezerController::start(freezer, runtime.handle().clone(), config).unwrap();
            runtime.block_on(async {
                assert!(controller.append(vec![]).await.is_err());
                assert!(controller.append(vec![block(1); 9]).await.is_err());
                assert_eq!(controller.append(vec![block(42)]).await.unwrap(), 1..2);
                assert!(controller.retrieve_many(vec![1], 8193).await.is_err());
                assert!(controller.retrieve_many(vec![1], 1).await.is_err());
                assert_eq!(
                    controller.retrieve_many(vec![0, 1, 2], 8192).await.unwrap(),
                    [None, Some(block(42).as_slice().to_vec()), None]
                );
                let mut writes = tokio::task::JoinSet::new();
                for number in 1..=16 {
                    let writer = controller.clone();
                    writes.spawn(async move {
                        (
                            number,
                            writer.append(vec![block(number)]).await.unwrap().start,
                        )
                    });
                }
                let mut ordinals = Vec::new();
                while let Some(result) = writes.join_next().await {
                    let (number, ordinal) = result.unwrap();
                    assert_eq!(
                        controller.retrieve(ordinal).unwrap(),
                        Some(block(number).as_slice().to_vec())
                    );
                    ordinals.push(ordinal);
                }
                ordinals.sort_unstable();
                assert_eq!(ordinals, (2..18).collect::<Vec<_>>());
                controller.shutdown().await.unwrap();
                controller.shutdown().await.unwrap();
                assert!(controller.append(vec![block(1)]).await.is_err());
                assert!(controller.retrieve_many(vec![1], 1024).await.is_err());
                assert_eq!(
                    controller.retrieve(1).unwrap(),
                    Some(block(42).as_slice().to_vec())
                );
            });
            drop(controller);
            let reopened = Freezer::open_in(directory.path()).unwrap();
            assert_eq!(reopened.number(), 18);
        }
    }

    #[test]
    fn cancelled_append_and_shutdown_still_drain_accepted_io() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let freezer = Freezer::open_in(directory.path()).unwrap();
        let controller = FreezerController::start(
            freezer.clone(),
            runtime.handle().clone(),
            FreezerServiceConfig {
                write_queue: 1,
                read_workers: 1,
                batch_records: 4,
                io_bytes: 8192,
            },
        )
        .unwrap();
        let first = block(1).into_view();
        runtime
            .block_on(controller.append(vec![first.data()]))
            .unwrap();
        let second = BlockBuilder::default()
            .number(2)
            .epoch(first.epoch())
            .parent_hash(first.hash())
            .build();
        let (entered, waiting) = sync_mpsc::channel();
        let (release, released) = sync_mpsc::channel();
        let writer = std::thread::spawn(move || {
            freezer.with_writer(|files| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                files.append(files.number(), second.data().as_slice())?;
                files.sync_all()
            })
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.block_on(async {
            let appender = controller.clone();
            let append = tokio::spawn(async move { appender.append(vec![block(30)]).await });
            while controller.status().accepted_requests == 0 {
                tokio::task::yield_now().await;
            }
            assert!(controller.0.bytes.available_permits() < 8192);
            // Both read entry points work while the serial writer is blocked.
            assert!(controller.retrieve(1).unwrap().is_some());
            assert!(controller.retrieve_many(vec![1], 1024).await.unwrap()[0].is_some());
            append.abort();
            assert!(append.await.unwrap_err().is_cancelled());
            let closer = controller.clone();
            let shutdown = tokio::spawn(async move { closer.shutdown().await });
            while !controller.status().shutting_down {
                tokio::task::yield_now().await;
            }
            assert!(!shutdown.is_finished());
            shutdown.abort();
            assert!(shutdown.await.unwrap_err().is_cancelled());
            release.send(()).unwrap();
            controller.shutdown().await.unwrap();
            assert_eq!(controller.number(), 4);
            assert_eq!(
                controller.retrieve(3).unwrap(),
                Some(block(30).as_slice().to_vec())
            );
            assert_eq!(controller.status().accepted_requests, 0);
        });
        writer.join().unwrap().unwrap();
        drop(controller);
        let reopened = Freezer::open_in(directory.path()).unwrap();
        assert_eq!(
            reopened.retrieve(3).unwrap(),
            Some(block(30).as_slice().to_vec())
        );
    }

    #[test]
    fn shutdown_rejects_reads_waiting_for_admission() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let controller = FreezerController::start(
            Freezer::open_in(directory.path()).unwrap(),
            runtime.handle().clone(),
            FreezerServiceConfig {
                read_workers: 1,
                ..Default::default()
            },
        )
        .unwrap();
        runtime.block_on(async {
            let reserved = controller.0.read_slots.acquire().await.unwrap();
            let reader = controller.clone();
            let waiting = tokio::spawn(async move { reader.retrieve_many(vec![1], 1024).await });
            while controller.0.bytes.available_permits() == controller.0.config.io_bytes as usize {
                tokio::task::yield_now().await;
            }
            assert_eq!(controller.status().accepted_requests, 0);
            controller.shutdown().await.unwrap();
            assert!(waiting.await.unwrap().is_err());
            drop(reserved);
        });
    }

    #[test]
    fn dropping_last_controller_drains_accepted_append() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let freezer = Freezer::open_in(directory.path()).unwrap();
        let controller = FreezerController::start(
            freezer.clone(),
            runtime.handle().clone(),
            FreezerServiceConfig::default(),
        )
        .unwrap();
        let (entered, waiting) = sync_mpsc::channel();
        let (release, released) = sync_mpsc::channel();
        let writer = std::thread::spawn(move || {
            freezer.with_writer(|files| {
                entered.send(()).unwrap();
                let _ = released.recv();
                files.append(files.number(), block(1).as_slice())?;
                files.sync_all()
            })
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.block_on(async move {
            let appender = controller.clone();
            let append = tokio::spawn(async move { appender.append(vec![block(99)]).await });
            while controller.status().accepted_requests == 0 {
                tokio::task::yield_now().await;
            }
            append.abort();
            assert!(append.await.unwrap_err().is_cancelled());
            let mut done = controller.0.writer_done.clone();
            drop(controller);
            release.send(()).unwrap();
            loop {
                if let Some(result) = done.borrow().clone() {
                    result.unwrap();
                    break;
                }
                done.changed().await.unwrap();
            }
        });
        writer.join().unwrap().unwrap();
        let reopened = Freezer::open_in(directory.path()).unwrap();
        assert_eq!(reopened.number(), 3);
        assert_eq!(
            reopened.retrieve(2).unwrap(),
            Some(block(99).as_slice().to_vec())
        );
    }
}
