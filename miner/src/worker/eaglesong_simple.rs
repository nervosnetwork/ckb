use super::{Worker, WorkerMessage};
use crate::Work;
use ckb_app_config::ExtraHashFunction;
use ckb_channel::{Receiver, Sender};
use ckb_hash::blake2b_256;
use ckb_logger::{debug, error};
use ckb_pow::pow_message;
use ckb_types::{U256, packed::Byte32};
use eaglesong::eaglesong;
use indicatif::ProgressBar;
use std::thread;
use std::time::{Duration, Instant};

pub struct EaglesongSimple {
    start: bool,
    pow_work: Option<(Byte32, Work)>,
    target: U256,
    nonce_tx: Sender<(Byte32, Work, u128)>,
    worker_rx: Receiver<WorkerMessage>,
    nonces_found: u128,
    pub(crate) extra_hash_function: Option<ExtraHashFunction>,
}

impl EaglesongSimple {
    pub fn new(
        nonce_tx: Sender<(Byte32, Work, u128)>,
        worker_rx: Receiver<WorkerMessage>,
        extra_hash_function: Option<ExtraHashFunction>,
    ) -> Self {
        Self {
            start: true,
            pow_work: None,
            target: U256::zero(),
            nonce_tx,
            worker_rx,
            nonces_found: 0,
            extra_hash_function,
        }
    }

    fn poll_worker_message(&mut self) {
        while let Ok(msg) = self.worker_rx.try_recv() {
            match msg {
                WorkerMessage::NewWork {
                    pow_hash,
                    work,
                    target,
                } => {
                    self.pow_work = Some((pow_hash, work));
                    self.target = target;
                }
                WorkerMessage::Stop => {
                    self.start = false;
                }
                WorkerMessage::Start => {
                    self.start = true;
                }
            }
        }
    }

    fn solve(&mut self, pow_hash: Byte32, work: Work, nonce: u128) {
        debug!("Solved. pow_hash {}, nonce {:?}", pow_hash, nonce);
        let input = pow_message(&pow_hash, nonce);
        let output = {
            let mut output_tmp = [0u8; 32];
            eaglesong(&input, &mut output_tmp);
            match self.extra_hash_function {
                Some(ExtraHashFunction::Blake2b) => blake2b_256(output_tmp),
                None => output_tmp,
            }
        };
        if U256::from_big_endian(&output[..]).expect("bound checked") <= self.target {
            debug!(
                "Send newly found nonce, pow_hash {}, nonce {:?}",
                pow_hash, nonce
            );
            if let Err(err) = self.nonce_tx.send((pow_hash, work, nonce)) {
                error!("nonce_tx send error {:?}", err);
            }
            self.nonces_found += 1;
        }
    }
}

const STATE_UPDATE_DURATION_MILLIS: u128 = 500;

impl Worker for EaglesongSimple {
    fn run<G: FnMut() -> u128>(&mut self, mut rng: G, progress_bar: ProgressBar) {
        let mut state_update_counter = 0usize;
        let mut start = Instant::now();
        loop {
            self.poll_worker_message();
            if self.start {
                if let Some((pow_hash, work)) = self.pow_work.clone() {
                    self.solve(pow_hash, work, rng());
                    state_update_counter += 1;

                    let elapsed = Instant::now().saturating_duration_since(start);
                    if elapsed.as_millis() > STATE_UPDATE_DURATION_MILLIS {
                        let elapsed_nanos: f64 = (elapsed.as_secs() * 1_000_000_000
                            + u64::from(elapsed.subsec_nanos()))
                            as f64
                            / 1_000_000_000.0;
                        progress_bar.set_message(format!(
                            "hash rate: {:>10.3} / nonces found: {:>10}",
                            state_update_counter as f64 / elapsed_nanos,
                            self.nonces_found,
                        ));
                        progress_bar.inc(1);
                        state_update_counter = 0;
                        start = Instant::now();
                    }
                }
            } else {
                // reset state and sleep
                state_update_counter = 0;
                start = Instant::now();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
