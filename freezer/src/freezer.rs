use crate::freezer_files::FreezerFiles;
use crate::internal_error;
use ckb_error::Error;
use ckb_shared::shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::ChainStore;
use ckb_types::{
    core::{service, BlockNumber, EpochNumber},
    prelude::*,
};
use fs2::FileExt;
use std::cmp;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const FREEZER_INTERVAL: Duration = Duration::from_secs(60);
const THRESHOLD_EPOCH: EpochNumber = 2;
const MAX_LIMIT: BlockNumber = 30_000;
const LOCKNAME: &str = "FLOCK";

pub struct Freezer {
    pub files: FreezerFiles,
    pub shared: Shared,
    pub lock: File,
}

pub struct FreezerClose {
    stop: StopHandler<()>,
}

impl Drop for FreezerClose {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

pub struct FreezerService {
    path: PathBuf,
    shared: Shared,
}

impl FreezerService {
    pub fn new(path: PathBuf, shared: Shared) -> FreezerService {
        FreezerService { path, shared }
    }

    pub fn start(self) -> FreezerClose {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(service::SIGNAL_CHANNEL_SIZE);
        let thread = thread::Builder::new()
            .spawn(move || {
                let mut freezer = Freezer::open(self.path, self.shared).unwrap_or_else(|e| {
                    panic!("Freezer open failed {}", e);
                });
                loop {
                    match signal_receiver.recv_timeout(FREEZER_INTERVAL) {
                        Err(_) => {
                            if let Err(e) = freezer.freeze() {
                                ckb_logger::error!("Freezer error {}", e);
                            }
                        }
                        Ok(_) => {
                            ckb_logger::info!("Freezer closing");
                            break;
                        }
                    }
                }
            })
            .expect("Start FreezerService failed");

        let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), Some(thread));
        FreezerClose { stop }
    }
}

impl Freezer {
    pub fn open(path: PathBuf, shared: Shared) -> Result<Freezer, Error> {
        let lock_path = path.join(LOCKNAME);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(internal_error)?;
        lock.try_lock_exclusive().map_err(internal_error)?;
        let files = FreezerFiles::open(path)?;
        Ok(Freezer {
            files,
            shared,
            lock,
        })
    }

    pub fn freeze(&mut self) -> Result<(), Error> {
        if let Some(threshold) = self.threshold() {
            assert!(self.files.number >= 1);
            for number in self.files.number..threshold {
                if let Some(block) = self
                    .shared
                    .store()
                    .get_block_hash(number)
                    .and_then(|hash| self.shared.store().get_archived_block(&hash))
                {
                    self.append(number, block.as_slice())?;
                    ckb_logger::debug!("freezer block append {}", number);
                } else {
                    ckb_logger::error!("freezer block missing {}", number);
                    break;
                }
            }
            self.files.sync_all()?;
        }
        Ok(())
    }

    pub fn threshold(&self) -> Option<BlockNumber> {
        let snapshot = self.shared.snapshot();
        let current_epoch = snapshot.epoch_ext().number();

        ckb_logger::debug!("freezer current_epoch {}", current_epoch);

        if current_epoch <= THRESHOLD_EPOCH {
            ckb_logger::debug!("freezer loaf");
            return None;
        }

        let limit_block_hash = snapshot
            .get_epoch_index(current_epoch + 1 - THRESHOLD_EPOCH)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .expect("get_epoch_ext")
            .last_block_hash_in_previous_epoch();

        Some(cmp::min(
            snapshot
                .get_block_number(&limit_block_hash)
                .expect("get_block_number"),
            self.files.number + MAX_LIMIT,
        ))
    }

    fn append(&mut self, number: u64, data: &[u8]) -> Result<(), Error> {
        self.files.append(number, data)
    }
}

impl Drop for Freezer {
    fn drop(&mut self) {
        if let Err(e) = self.lock.unlock() {
            ckb_logger::warn!("freezer filelock unlock {}", e);
        }
    }
}
