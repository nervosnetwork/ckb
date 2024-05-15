use crate::{LonelyBlockHash, UnverifiedBlock};
use ckb_channel::{Receiver, Sender};
use ckb_logger::{debug, error, info};
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_types::core::HeaderView;
use crossbeam::select;
use either::Either;
use std::cell::Cell;
use std::sync::Arc;

pub(crate) struct PreloadUnverifiedBlocksChannel {
    shared: Shared,
    preload_unverified_rx: Receiver<LonelyBlockHash>,

    unverified_block_tx: Sender<UnverifiedBlock>,

    stop_rx: Receiver<()>,

    // after we load a block from store, we put block.parent_header into this cell
    prev_header: Cell<HeaderView>,
}

impl PreloadUnverifiedBlocksChannel {
    pub(crate) fn new(
        shared: Shared,
        preload_unverified_rx: Receiver<LonelyBlockHash>,
        unverified_block_tx: Sender<UnverifiedBlock>,
        stop_rx: Receiver<()>,
    ) -> Self {
        let tip_hash = shared.snapshot().tip_hash();

        let tip_header = shared
            .store()
            .get_block_header(&tip_hash)
            .expect("must get tip header");

        PreloadUnverifiedBlocksChannel {
            shared,
            preload_unverified_rx,
            unverified_block_tx,
            stop_rx,
            prev_header: Cell::new(tip_header),
        }
    }

    pub(crate) fn start(&self) {
        loop {
            select! {
                recv(self.preload_unverified_rx) -> msg => match msg {
                    Ok(preload_unverified_block_task) =>{
                        self.preload_unverified_channel(preload_unverified_block_task);
                    },
                    Err(err) =>{
                        error!("recv preload_task_rx failed, err: {:?}", err);
                        break;
                    }
                },
                recv(self.stop_rx) -> _ => {
                    info!("preload_unverified_blocks thread received exit signal, exit now");
                    break;
                }
            }
        }
    }

    fn preload_unverified_channel(&self, task: LonelyBlockHash) {
        let block_number = task.number();
        let block_hash = task.hash();
        let unverified_block: UnverifiedBlock = self.load_full_unverified_block_by_hash(task);

        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_chain_unverified_block_ch_len
                .set(self.unverified_block_tx.len() as i64)
        };

        if self.unverified_block_tx.send(unverified_block).is_err() {
            info!(
                "send unverified_block to unverified_block_tx failed, the receiver has been closed"
            );
        } else {
            debug!("preload unverified block {}-{}", block_number, block_hash,);
        }
    }

    fn load_full_unverified_block_by_hash(&self, task: LonelyBlockHash) -> UnverifiedBlock {
        let _trace_timecost = ckb_metrics::handle()
            .map(|metrics| metrics.ckb_chain_load_full_unverified_block.start_timer());

        let LonelyBlockHash {
            block_number_and_hash,
            parent_hash,
            epoch_number: _epoch_number,
            switch,
            verify_callback,
        } = task;

        let block = {
            match block_number_and_hash {
                Either::Left(number_and_hash) => {
                    let block_view = self
                        .shared
                        .store()
                        .get_block(&number_and_hash.hash())
                        .expect("block stored");
                    Arc::new(block_view)
                }
                Either::Right(block) => block,
            }
        };

        let parent_header = {
            let prev_header = self.prev_header.replace(block.header());
            if prev_header.hash() == parent_hash {
                prev_header
            } else {
                self.shared
                    .store()
                    .get_block_header(&parent_hash)
                    .expect("parent header stored")
            }
        };

        UnverifiedBlock {
            block,
            switch,
            verify_callback,
            parent_header,
        }
    }
}
