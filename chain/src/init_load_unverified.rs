use crate::{ChainController, LonelyBlock};
use ckb_channel::{select, Receiver};
use ckb_db::{Direction, IteratorMode};
use ckb_db_schema::COLUMN_NUMBER_HASH;
use ckb_logger::info;
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_types::core::{BlockNumber, BlockView};
use ckb_types::packed;
use ckb_types::prelude::{Entity, FromSliceShouldBeOk, Pack, Reader};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(crate) struct InitLoadUnverified {
    shared: Shared,
    chain_controller: ChainController,
    is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,

    stop_rx: Receiver<()>,
}

impl InitLoadUnverified {
    pub(crate) fn new(
        shared: Shared,
        chain_controller: ChainController,
        stop_rx: Receiver<()>,
        is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,
    ) -> Self {
        InitLoadUnverified {
            shared,
            chain_controller,
            is_verifying_unverified_blocks_on_startup,
            stop_rx,
        }
    }
    fn print_unverified_blocks_count(&self) {
        let tip_number: BlockNumber = self.shared.snapshot().tip_number();
        let mut check_unverified_number = tip_number + 1;
        let mut unverified_block_count = 0;
        loop {
            // start checking `check_unverified_number` have COLUMN_NUMBER_HASH value in db?
            let unverified_hashes: Vec<packed::Byte32> =
                self.find_unverified_block_hashes(check_unverified_number);
            unverified_block_count += unverified_hashes.len();
            if unverified_hashes.is_empty() {
                info!(
                    "found {} unverified blocks, verifying...",
                    unverified_block_count
                );
                break;
            }
            check_unverified_number += 1;
        }
    }

    fn find_unverified_block_hashes(&self, check_unverified_number: u64) -> Vec<packed::Byte32> {
        let pack_number: packed::Uint64 = check_unverified_number.pack();
        let prefix = pack_number.as_slice();

        let unverified_hashes: Vec<packed::Byte32> = self
            .shared
            .store()
            .get_iter(
                COLUMN_NUMBER_HASH,
                IteratorMode::From(prefix, Direction::Forward),
            )
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key_number_hash, _v)| {
                let reader =
                    packed::NumberHashReader::from_slice_should_be_ok(key_number_hash.as_ref());
                let unverified_block_hash = reader.block_hash().to_entity();
                unverified_block_hash
            })
            .collect::<Vec<packed::Byte32>>();
        unverified_hashes
    }

    pub(crate) fn start(&self) {
        info!(
            "finding unverified blocks, current tip: {}-{}",
            self.shared.snapshot().tip_number(),
            self.shared.snapshot().tip_hash()
        );
        self.print_unverified_blocks_count();

        self.find_and_verify_unverified_blocks();

        self.is_verifying_unverified_blocks_on_startup
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn find_and_verify_unverified_blocks(&self) {
        let tip_number: BlockNumber = self.shared.snapshot().tip_number();
        let mut check_unverified_number = tip_number + 1;

        loop {
            select! {
                recv(self.stop_rx) -> _msg => {
                    info!("init_unverified_blocks thread received exit signal, exit now");
                    break;
                },
                default => {}
            }

            // start checking `check_unverified_number` have COLUMN_NUMBER_HASH value in db?
            let unverified_hashes: Vec<packed::Byte32> =
                self.find_unverified_block_hashes(check_unverified_number);

            if unverified_hashes.is_empty() {
                if check_unverified_number == tip_number + 1 {
                    info!("no unverified blocks found.");
                } else {
                    info!(
                        "found and verify unverified blocks finish, current tip: {}-{}",
                        self.shared.snapshot().tip_number(),
                        self.shared.snapshot().tip_header()
                    );
                }
                return;
            }

            for unverified_hash in unverified_hashes {
                let unverified_block: BlockView = self
                    .shared
                    .store()
                    .get_block(&unverified_hash)
                    .expect("unverified block must be in db");
                self.chain_controller
                    .asynchronous_process_lonely_block(LonelyBlock {
                        block: Arc::new(unverified_block),
                        switch: None,
                        verify_callback: None,
                    });
            }

            check_unverified_number += 1;
        }
    }
}
