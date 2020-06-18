use crate::freezer_files::FreezerFiles;
use crate::internal_error;
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use ckb_util::Mutex;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const LOCKNAME: &str = "FLOCK";

struct Inner {
    pub(crate) files: Mutex<FreezerFiles>,
    pub(crate) lock: File,
}

#[derive(Clone)]
pub struct Freezer {
    inner: Arc<Inner>,
    number: Arc<AtomicU64>,
}

// pub struct FreezerService<S> {
//     path: PathBuf,
//     store: S,
// }

// impl<'a, S: ChainStore<'a>> FreezerService<'a, S> {
//     pub fn new(path: PathBuf, store: S) -> FreezerService {
//         FreezerService { path, store }
//     }

//     pub fn start(self) -> FreezerClose {
//         let (signal_sender, signal_receiver) =
//             crossbeam_channel::bounded::<()>(service::SIGNAL_CHANNEL_SIZE);
//         let thread = thread::Builder::new()
//             .spawn(move || {
//                 let mut freezer = Freezer::open(self.path).unwrap_or_else(|e| {
//                     panic!("Freezer open failed {}", e);
//                 });
//                 loop {
//                     match signal_receiver.recv_timeout(FREEZER_INTERVAL) {
//                         Err(_) => {
//                             if let Err(e) = freezer.freeze() {
//                                 ckb_logger::error!("Freezer error {}", e);
//                             }
//                         }
//                         Ok(_) => {
//                             ckb_logger::info!("Freezer closing");
//                             break;
//                         }
//                     }
//                 }
//             })
//             .expect("Start FreezerService failed");

//         let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), thread);
//         FreezerClose { stop }
//     }
// }

impl Freezer {
    pub fn open(path: PathBuf) -> Result<Freezer, Error> {
        let lock_path = path.join(LOCKNAME);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(internal_error)?;
        lock.try_lock_exclusive().map_err(internal_error)?;
        let files = FreezerFiles::open(path)?;
        let number = Arc::clone(&files.number);
        let inner = Inner {
            files: Mutex::new(files),
            lock,
        };
        Ok(Freezer {
            inner: Arc::new(inner),
            number,
        })
    }

    pub fn freeze<F>(&self, threshold: BlockNumber, callback: F) -> Result<(), Error>
    where
        F: Fn(BlockNumber) -> Option<packed::ArchivedBlock>,
    {
        let number = self.number();
        let mut guard = self.inner.files.lock();
        for number in number..threshold {
            if let Some(block) = callback(number) {
                guard.append(number, block.as_slice())?;
                ckb_logger::debug!("freezer block append {}", number);
            } else {
                ckb_logger::error!("freezer block missing {}", number);
                break;
            }
        }
        guard.sync_all()
    }

    pub fn retrieve(&self, number: BlockNumber) -> Result<Vec<u8>, Error> {
        self.inner.files.lock().retrieve(number)
    }

    pub fn number(&self) -> BlockNumber {
        self.number.load(Ordering::SeqCst)
    }
}

// pub fn freeze(&mut self) -> Result<(), Error> {
//     if let Some(threshold) = self.threshold() {
//         assert!(self.files.number >= 1);
//         for number in self.files.number..threshold {
//             if let Some(block) = self
//                 .shared
//                 .store()
//                 .get_block_hash(number)
//                 .and_then(|hash| self.shared.store().get_archived_block(&hash))
//             {
//                 self.append(number, block.as_slice())?;
//                 ckb_logger::debug!("freezer block append {}", number);
//             } else {
//                 ckb_logger::error!("freezer block missing {}", number);
//                 break;
//             }
//         }
//         self.files.sync_all()?;
//     }
//     Ok(())
// }

// pub fn threshold(&self) -> Option<BlockNumber> {
//     let snapshot = self.shared.snapshot();
//     let current_epoch = snapshot.epoch_ext().number();

//     ckb_logger::debug!("freezer current_epoch {}", current_epoch);

//     if current_epoch <= THRESHOLD_EPOCH {
//         ckb_logger::debug!("freezer loaf");
//         return None;
//     }

//     let limit_block_hash = snapshot
//         .get_epoch_index(current_epoch + 1 - THRESHOLD_EPOCH)
//         .and_then(|index| snapshot.get_epoch_ext(&index))
//         .expect("get_epoch_ext")
//         .last_block_hash_in_previous_epoch();

//     Some(cmp::min(
//         snapshot
//             .get_block_number(&limit_block_hash)
//             .expect("get_block_number"),
//         self.files.number + MAX_LIMIT,
//     ))
// }
