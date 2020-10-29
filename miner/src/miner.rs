use crate::client::Client;
use crate::worker::{start_worker, WorkerController, WorkerMessage};
use crate::Work;
use ckb_app_config::MinerWorkerConfig;
use ckb_channel::{select, unbounded, Receiver};
use ckb_logger::{debug, error, info};
use ckb_pow::PowEngine;
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
    /// TODO(doc): @quake
    pub pow: Arc<dyn PowEngine>,
    /// TODO(doc): @quake
    pub client: Client,
    /// TODO(doc): @quake
    pub works: LruCache<Byte32, Work>,
    /// TODO(doc): @quake
    pub worker_controllers: Vec<WorkerController>,
    /// TODO(doc): @quake
    pub work_rx: Receiver<Work>,
    /// TODO(doc): @quake
    pub nonce_rx: Receiver<(Byte32, u128)>,
    /// TODO(doc): @quake
    pub pb: ProgressBar,
    /// TODO(doc): @quake
    pub nonces_found: u128,
    /// TODO(doc): @quake
    pub stderr_is_tty: bool,
    /// TODO(doc): @quake
    pub limit: u128,
}

impl Miner {
    /// TODO(doc): @quake
    pub fn new(
        pow: Arc<dyn PowEngine>,
        client: Client,
        work_rx: Receiver<Work>,
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

        let stderr_is_tty = console::Term::stderr().is_term();

        thread::spawn(move || {
            mp.join().expect("MultiProgress join failed");
        });

        Miner {
            works: LruCache::new(WORK_CACHE_SIZE),
            nonces_found: 0,
            pow,
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
    // remove `allow` tag when https://github.com/crossbeam-rs/crossbeam/issues/404 is solved
    #[allow(clippy::zero_ptr, clippy::drop_copy)]
    pub fn run(&mut self) {
        loop {
            select! {
                recv(self.work_rx) -> msg => match msg {
                    Ok(work) => {
                        let pow_hash= work.block.header().calc_pow_hash();
                        let (target, _,) = compact_to_target(work.block.header().raw().compact_target().unpack());
                        self.works.put(pow_hash.clone(), work);
                        self.notify_workers(WorkerMessage::NewWork{pow_hash, target});
                    },
                    _ => {
                        error!("work_rx closed");
                        break;
                    },
                },
                recv(self.nonce_rx) -> msg => match msg {
                    Ok((pow_hash, nonce)) => {
                        self.submit_nonce(pow_hash, nonce);
                        if self.limit != 0 && self.nonces_found >= self.limit {
                            break;
                        }
                    },
                    _ => {
                        error!("nonce_rx closed");
                        break;
                    },
                }
            };
        }
    }

    fn submit_nonce(&mut self, pow_hash: Byte32, nonce: u128) {
        if let Some(work) = self.works.get(&pow_hash).cloned() {
            self.notify_workers(WorkerMessage::Stop);
            let raw_header = work.block.header().raw();
            let header = Header::new_builder()
                .raw(raw_header)
                .nonce(nonce.pack())
                .build();
            let block = work.block.as_builder().header(header).build().into_view();
            let block_hash = block.hash();
            if self.stderr_is_tty {
                debug!("Found! #{} {:#x}", block.number(), block_hash);
            } else {
                info!("Found! #{} {:#x}", block.number(), block_hash);
            }

            // submit block and poll new work
            {
                self.client
                    .submit_block(&work.work_id.to_string(), block.data());
                self.client.try_update_block_template();
                self.notify_workers(WorkerMessage::Start);
            }

            // draw progress bar
            {
                self.nonces_found += 1;
                self.pb
                    .println(format!("Found! #{} {:#x}", block.number(), block_hash));
                self.pb
                    .set_message(&format!("Total nonces found: {:>3}", self.nonces_found));
                self.pb.inc(1);
            }
        }
    }

    fn notify_workers(&self, message: WorkerMessage) {
        for controller in self.worker_controllers.iter() {
            controller.send_message(message.clone());
        }
    }
}
