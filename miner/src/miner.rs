use crate::client::{Client, Works};
use crate::worker::{start_worker, WorkerController, WorkerMessage};
use crate::Work;
use ckb_app_config::MinerWorkerConfig;
use ckb_channel::{select, unbounded, Receiver};
use ckb_logger::{debug, error, info};
use ckb_pow::PowEngine;
use ckb_stop_handler::broadcast_exit_signals;
use ckb_types::{
    packed::{Byte32, Header},
    prelude::*,
    utilities::compact_to_target,
};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use lru::LruCache;
use std::sync::Arc;
use std::thread;

const WORK_CACHE_SIZE: usize = 32;

/// TODO(doc): @quake
pub struct Miner {
    pub(crate) _pow: Arc<dyn PowEngine>,
    pub(crate) client: Client,
    /// Tasks's parent's hash that have already been submitted
    pub(crate) legacy_work: LruCache<Byte32, ()>,
    pub(crate) worker_controllers: Vec<WorkerController>,
    pub(crate) work_rx: Receiver<Works>,
    pub(crate) nonce_rx: Receiver<(Byte32, Work, u128)>,
    pub(crate) pb: ProgressBar,
    pub(crate) nonces_found: u128,
    pub(crate) stderr_is_tty: bool,
    pub(crate) limit: u128,
}

impl Miner {
    /// TODO(doc): @quake
    pub fn new(
        pow: Arc<dyn PowEngine>,
        client: Client,
        work_rx: Receiver<Works>,
        workers: &[MinerWorkerConfig],
        limit: u128,
    ) -> Miner {
        let (nonce_tx, nonce_rx) = unbounded();
        let mp = MultiProgress::new();

        let worker_controllers = workers
            .iter()
            .map(|config| start_worker(Arc::clone(&pow), config, nonce_tx.clone(), &mp))
            .collect();

        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::default_bar().template("{msg:.green}"));

        let stderr_is_tty = console::Term::stderr().features().is_attended();

        thread::spawn(move || {
            mp.join().expect("MultiProgress join failed");
        });

        Miner {
            legacy_work: LruCache::new(WORK_CACHE_SIZE),
            nonces_found: 0,
            _pow: pow,
            client,
            worker_controllers,
            work_rx,
            nonce_rx,
            pb,
            stderr_is_tty,
            limit,
        }
    }

    /// TODO(doc): @quake
    pub fn run(&mut self, stop_rx: Receiver<()>) {
        loop {
            select! {
                recv(self.work_rx) -> msg => match msg {
                    Ok(work) => {
                        match work {
                            Works::FailSubmit(hash) => {
                                self.legacy_work.pop(&hash);
                            },
                            Works::New(work) => self.notify_new_work(work),
                        }
                    },
                    _ => {
                        error!("work_rx closed");
                        break;
                    },
                },
                recv(self.nonce_rx) -> msg => match msg {
                    Ok((pow_hash, work, nonce)) => {
                        self.submit_nonce(pow_hash, work, nonce);
                        if self.limit != 0 && self.nonces_found >= self.limit {
                            debug!("miner nonce limit reached, terminate ...");
                            broadcast_exit_signals();
                        }
                    },
                    _ => {
                        error!("nonce_rx closed");
                        break;
                    },
                },
                recv(stop_rx) -> _msg => {
                    debug!("miner received exit signal, stopped");
                    break;
                }
            };
        }
    }

    fn notify_new_work(&mut self, work: Work) {
        let parent_hash = work.block.header().into_view().parent_hash();
        if !self.legacy_work.contains(&parent_hash) {
            let pow_hash = work.block.header().calc_pow_hash();
            let (target, _) =
                compact_to_target(work.block.header().raw().compact_target().unpack());
            self.notify_workers(WorkerMessage::NewWork {
                pow_hash,
                work,
                target,
            });
        }
    }

    fn submit_nonce(&mut self, pow_hash: Byte32, work: Work, nonce: u128) {
        self.notify_workers(WorkerMessage::Stop);
        let raw_header = work.block.header().raw();
        let header = Header::new_builder()
            .raw(raw_header)
            .nonce(nonce.pack())
            .build();
        let block = work
            .block
            .as_advanced_builder()
            .header(header.into_view())
            .build();
        let block_hash = block.hash();
        let parent_hash = block.parent_hash();

        if self.legacy_work.contains(&parent_hash) {
            debug!(
                "uncle {} pow_hash: {:#x}, header: {}",
                block.number(),
                pow_hash,
                block.header()
            );
            self.notify_workers(WorkerMessage::Start);
            return;
        } else {
            debug!(
                "block {} pow_hash: {:#x}, header: {}",
                block.number(),
                pow_hash,
                block.header()
            );
        }

        self.legacy_work.put(parent_hash, ());
        if self.stderr_is_tty {
            debug!("Found! #{} {:#x}", block.number(), block_hash);
        } else {
            info!("Found! #{} {:#x}", block.number(), block_hash);
        }

        // submit block and poll new work
        {
            if let Err(e) = self
                .client
                .submit_block(&work.work_id.to_string(), block.data())
            {
                self.legacy_work.pop(&block.parent_hash());
                error!("rpc call submit_block error: {:?}", e);
            }
            self.client.blocking_fetch_block_template();
            self.notify_workers(WorkerMessage::Start);
        }

        // draw progress bar
        {
            self.nonces_found += 1;
            self.pb
                .println(format!("Found! #{} {:#x}", block.number(), block_hash));
            self.pb
                .set_message(format!("Total nonces found: {:>3}", self.nonces_found));
            self.pb.inc(1);
        }
    }

    fn notify_workers(&self, message: WorkerMessage) {
        for controller in self.worker_controllers.iter() {
            controller.send_message(message.clone());
        }
    }
}
