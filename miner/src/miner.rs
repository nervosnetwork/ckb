use crate::client::Client;
use crate::config::WorkerConfig;
use crate::worker::{start_worker, WorkerController, WorkerMessage};
use crate::Work;
use ckb_core::block::BlockBuilder;
use ckb_core::header::Seal;
use ckb_logger::{debug, error};
use ckb_pow::PowEngine;
use ckb_util::Mutex;
use crossbeam_channel::{select, unbounded, Receiver};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::sync::Arc;
use std::thread;

const WORK_CACHE_SIZE: usize = 32;

pub struct Miner {
    pub pow: Arc<dyn PowEngine>,
    pub client: Client,
    pub works: Mutex<LruCache<H256, Work>>,
    pub worker_controllers: Vec<WorkerController>,
    pub work_rx: Receiver<Work>,
    pub seal_rx: Receiver<(H256, Seal)>,
    pub pb: ProgressBar,
    pub seals_found: u64,
}

impl Miner {
    pub fn new(
        pow: Arc<dyn PowEngine>,
        client: Client,
        work_rx: Receiver<Work>,
        workers: &[WorkerConfig],
    ) -> Miner {
        let (seal_tx, seal_rx) = unbounded();
        let mp = MultiProgress::new();

        let worker_controllers = workers
            .iter()
            .map(|config| start_worker(Arc::clone(&pow), config, seal_tx.clone(), &mp))
            .collect();

        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::default_bar().template("{msg:.green}"));

        thread::spawn(move || {
            mp.join().expect("MultiProgress join failed");
        });

        Miner {
            works: Mutex::new(LruCache::new(WORK_CACHE_SIZE)),
            seals_found: 0,
            pow,
            client,
            worker_controllers,
            work_rx,
            seal_rx,
            pb,
        }
    }

    pub fn run(&mut self) {
        loop {
            select! {
                recv(self.work_rx) -> msg => match msg {
                    Ok(work) => {
                        let pow_hash = work.block.header().pow_hash();
                        self.works.lock().insert(pow_hash.clone(), work);
                        self.notify_workers(WorkerMessage::NewWork(pow_hash));
                    },
                    _ => {
                        error!("work_rx closed");
                        break;
                    },
                },
                recv(self.seal_rx) -> msg => match msg {
                    Ok((pow_hash, seal)) => self.check_seal(pow_hash, seal),
                    _ => {
                        error!("seal_rx closed");
                        break;
                    },
                }
            };
        }
    }

    fn check_seal(&mut self, pow_hash: H256, seal: Seal) {
        if let Some(work) = self.works.lock().get_refresh(&pow_hash) {
            if self
                .pow
                .verify_proof_difficulty(&seal.proof(), &work.block.header().difficulty())
            {
                self.notify_workers(WorkerMessage::Stop);
                let raw_header = work.block.header().raw().to_owned();
                let block = BlockBuilder::from_block(work.block.clone())
                    .header(raw_header.with_seal(seal))
                    .build();

                debug!(
                    "Found! #{} {:#x}",
                    block.header().number(),
                    block.header().hash(),
                );

                // submit block and poll new work
                {
                    self.client.submit_block(&work.work_id.to_string(), &block);
                    self.client.try_update_block_template();
                    self.notify_workers(WorkerMessage::Start);
                }

                // draw progress bar
                {
                    self.seals_found += 1;
                    self.pb.println(format!(
                        "Found! #{} {:#x}",
                        block.header().number(),
                        block.header().hash()
                    ));
                    self.pb
                        .set_message(&format!("Total seals found: {:>3}", self.seals_found));
                    self.pb.inc(1);
                }
            }
        }
    }

    fn notify_workers(&self, message: WorkerMessage) {
        for controller in self.worker_controllers.iter() {
            controller.send_message(message.clone());
        }
    }
}
