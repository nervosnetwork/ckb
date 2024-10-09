use crate::utils::orphan_block_pool::EXPIRED_EPOCH;
use crate::{ChainController, LonelyBlock};
use ckb_constant::sync::BLOCK_DOWNLOAD_WINDOW;
use ckb_db::{Direction, IteratorMode};
use ckb_db_schema::COLUMN_NUMBER_HASH;
use ckb_logger::info;
use ckb_shared::Shared;
use ckb_stop_handler::has_received_stop_signal;
use ckb_store::ChainStore;
use ckb_types::core::{BlockNumber, BlockView};
use ckb_types::packed;
use ckb_types::prelude::{Entity, FromSliceShouldBeOk, Pack, Reader};
use std::cmp;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(crate) struct InitLoadUnverified {
    shared: Shared,
    chain_controller: ChainController,
    is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,
}

impl InitLoadUnverified {
    pub(crate) fn new(
        shared: Shared,
        chain_controller: ChainController,
        is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,
    ) -> Self {
        InitLoadUnverified {
            shared,
            chain_controller,
            is_verifying_unverified_blocks_on_startup,
        }
    }

    fn find_unverified_block_hashes(&self, check_unverified_number: u64) -> Vec<packed::Byte32> {
        let pack_number: packed::Uint64 = check_unverified_number.pack();
        let prefix = pack_number.as_slice();

        // If a block has `COLUMN_NUMBER_HASH` but not `BlockExt`,
        // it indicates an unverified block inserted during the last shutdown.
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
            .filter(|hash| self.shared.store().get_block_ext(hash).is_none())
            .collect::<Vec<packed::Byte32>>();
        unverified_hashes
    }

    pub(crate) fn start(&self) {
        info!(
            "finding unverified blocks, current tip: {}-{}",
            self.shared.snapshot().tip_number(),
            self.shared.snapshot().tip_hash()
        );

        self.find_and_verify_unverified_blocks();

        self.is_verifying_unverified_blocks_on_startup
            .store(false, std::sync::atomic::Ordering::Release);
        info!("find unverified blocks finished");
    }

    fn find_unverified_blocks<F>(&self, f: F)
    where
        F: Fn(&packed::Byte32),
    {
        let tip_number: BlockNumber = self.shared.snapshot().tip_number();
        let start_check_number = cmp::max(
            1,
            tip_number.saturating_sub(EXPIRED_EPOCH * self.shared.consensus().max_epoch_length()),
        );
        let end_check_number = tip_number + BLOCK_DOWNLOAD_WINDOW * 10;

        for check_unverified_number in start_check_number..=end_check_number {
            if has_received_stop_signal() {
                info!("init_unverified_blocks thread received exit signal, exit now");
                return;
            }

            // start checking `check_unverified_number` have COLUMN_NUMBER_HASH value in db?
            let unverified_hashes: Vec<packed::Byte32> =
                self.find_unverified_block_hashes(check_unverified_number);

            if check_unverified_number > tip_number && unverified_hashes.is_empty() {
                info!(
                    "no unverified blocks found after tip, current tip: {}-{}",
                    tip_number,
                    self.shared.snapshot().tip_hash()
                );
                return;
            }

            for unverified_hash in unverified_hashes {
                f(&unverified_hash);
            }
        }
    }

    fn find_and_verify_unverified_blocks(&self) {
        self.find_unverified_blocks(|unverified_hash| {
            let unverified_block: BlockView = self
                .shared
                .store()
                .get_block(unverified_hash)
                .expect("unverified block must be in db");

            if has_received_stop_signal() {
                return;
            }

            self.chain_controller
                .asynchronous_process_lonely_block(LonelyBlock {
                    block: Arc::new(unverified_block),
                    switch: None,
                    verify_callback: None,
                });
        });
    }
}
