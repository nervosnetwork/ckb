//! CKB chain service.
#![allow(missing_docs)]

use crate::consume_unverified::ConsumeUnverifiedBlocks;
use crate::utils::orphan_block_pool::OrphanBlockPool;
use crate::{ChainController, LonelyBlock, LonelyBlockHash, ProcessBlockRequest};
use ckb_channel::{self as channel, select, Receiver, SendError, Sender};
use ckb_constant::sync::BLOCK_DOWNLOAD_WINDOW;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{self, debug, error, info, warn};
use ckb_shared::block_status::BlockStatus;
use ckb_shared::shared::Shared;
use ckb_shared::ChainServicesBuilder;
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread};
use ckb_types::core::{service::Request, BlockView};
use ckb_verification::{BlockVerifier, NonContextualBlockTxsVerifier};
use ckb_verification_traits::Verifier;
use std::sync::Arc;
use std::thread;

const ORPHAN_BLOCK_SIZE: usize = (BLOCK_DOWNLOAD_WINDOW * 2) as usize;

pub fn start_chain_services(builder: ChainServicesBuilder) -> ChainController {
    let orphan_blocks_broker = Arc::new(OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE));

    let (truncate_block_tx, truncate_block_rx) = channel::bounded(1);

    let (unverified_queue_stop_tx, unverified_queue_stop_rx) = ckb_channel::bounded::<()>(1);
    let (unverified_tx, unverified_rx) =
        channel::bounded::<LonelyBlockHash>(BLOCK_DOWNLOAD_WINDOW as usize * 3);

    let consumer_unverified_thread = thread::Builder::new()
        .name("consume_unverified_blocks".into())
        .spawn({
            let shared = builder.shared.clone();
            move || {
                let consume_unverified = ConsumeUnverifiedBlocks::new(
                    shared,
                    unverified_rx,
                    truncate_block_rx,
                    builder.proposal_table,
                    unverified_queue_stop_rx,
                );

                consume_unverified.start();
            }
        })
        .expect("start unverified_queue consumer thread should ok");

    let (lonely_block_tx, lonely_block_rx) =
        channel::bounded::<LonelyBlock>(BLOCK_DOWNLOAD_WINDOW as usize);

    let (search_orphan_pool_stop_tx, search_orphan_pool_stop_rx) = ckb_channel::bounded::<()>(1);

    let search_orphan_pool_thread = thread::Builder::new()
        .name("consume_orphan_blocks".into())
        .spawn({
            let orphan_blocks_broker = Arc::clone(&orphan_blocks_broker);
            let shared = builder.shared.clone();
            use crate::consume_orphan::ConsumeOrphan;
            move || {
                let consume_orphan = ConsumeOrphan::new(
                    shared,
                    orphan_blocks_broker,
                    unverified_tx,
                    lonely_block_rx,
                    search_orphan_pool_stop_rx,
                );
                consume_orphan.start();
            }
        })
        .expect("start search_orphan_pool thread should ok");

    let (process_block_tx, process_block_rx) = channel::bounded(BLOCK_DOWNLOAD_WINDOW as usize);

    let chain_service: ChainService =
        ChainService::new(builder.shared, process_block_rx, lonely_block_tx);
    let chain_service_thread = thread::Builder::new()
        .name("ChainService".into())
        .spawn({
            move || {
                chain_service.start_process_block();

                if let Err(SendError(_)) = search_orphan_pool_stop_tx.send(()) {
                    warn!("trying to notify search_orphan_pool thread to stop, but search_orphan_pool_stop_tx already closed")
                }
                let _ = search_orphan_pool_thread.join();

                if let Err(SendError(_))= unverified_queue_stop_tx.send(()){
                    warn!("trying to notify consume unverified thread to stop, but unverified_queue_stop_tx already closed");
                }
                let _ = consumer_unverified_thread.join();
            }
        })
        .expect("start chain_service thread should ok");
    register_thread("ChainServices", chain_service_thread);

    ChainController::new(process_block_tx, truncate_block_tx, orphan_blocks_broker)
}

/// Chain background service to receive LonelyBlock and only do `non_contextual_verify`
#[derive(Clone)]
pub(crate) struct ChainService {
    shared: Shared,

    process_block_rx: Receiver<ProcessBlockRequest>,

    lonely_block_tx: Sender<LonelyBlock>,
}
impl ChainService {
    /// Create a new ChainService instance with shared.
    pub(crate) fn new(
        shared: Shared,
        process_block_rx: Receiver<ProcessBlockRequest>,

        lonely_block_tx: Sender<LonelyBlock>,
    ) -> ChainService {
        ChainService {
            shared,
            process_block_rx,
            lonely_block_tx,
        }
    }

    /// Receive block from `process_block_rx` and do `non_contextual_verify`
    pub(crate) fn start_process_block(self) {
        let signal_receiver = new_crossbeam_exit_rx();

        loop {
            select! {
                recv(self.process_block_rx) -> msg => match msg {
                    Ok(Request { responder, arguments: lonely_block }) => {
                        // asynchronous_process_block doesn't interact with tx-pool,
                        // no need to pause tx-pool's chunk_process here.
                        let _trace_now = minstant::Instant::now();
                        self.asynchronous_process_block(lonely_block);
                        if let Some(handle) = ckb_metrics::handle(){
                            handle.ckb_chain_async_process_block_duration.observe(_trace_now.elapsed().as_secs_f64())
                        }
                        let _ = responder.send(());
                    },
                    _ => {
                        error!("process_block_receiver closed");
                        break;
                    },
                },
                recv(signal_receiver) -> _ => {
                    info!("ChainService received exit signal, exit now");
                    break;
                }
            }
        }
    }

    fn non_contextual_verify(&self, block: &BlockView) -> Result<(), Error> {
        let consensus = self.shared.consensus();
        BlockVerifier::new(consensus).verify(block).map_err(|e| {
            debug!("[process_block] BlockVerifier error {:?}", e);
            e
        })?;

        NonContextualBlockTxsVerifier::new(consensus)
            .verify(block)
            .map_err(|e| {
                debug!(
                    "[process_block] NonContextualBlockTxsVerifier error {:?}",
                    e
                );
                e
            })
            .map(|_| ())
    }

    // `self.non_contextual_verify` is very fast.
    fn asynchronous_process_block(&self, lonely_block: LonelyBlock) {
        let block_number = lonely_block.block().number();
        let block_hash = lonely_block.block().hash();
        // Skip verifying a genesis block if its hash is equal to our genesis hash,
        // otherwise, return error and ban peer.
        if block_number < 1 {
            if self.shared.genesis_hash() != block_hash {
                warn!(
                    "receive 0 number block: 0-{}, expect genesis hash: {}",
                    block_hash,
                    self.shared.genesis_hash()
                );
                self.shared
                    .insert_block_status(lonely_block.block().hash(), BlockStatus::BLOCK_INVALID);
                let error = InternalErrorKind::System
                    .other("Invalid genesis block received")
                    .into();
                lonely_block.execute_callback(Err(error));
            } else {
                warn!("receive 0 number block: 0-{}", block_hash);
                lonely_block.execute_callback(Ok(false));
            }
            return;
        }

        if lonely_block.switch().is_none()
            || matches!(lonely_block.switch(), Some(switch) if !switch.disable_non_contextual())
        {
            let result = self.non_contextual_verify(lonely_block.block());
            if let Err(err) = result {
                error!(
                    "block {}-{} verify failed: {:?}",
                    block_number, block_hash, err
                );
                self.shared
                    .insert_block_status(lonely_block.block().hash(), BlockStatus::BLOCK_INVALID);
                lonely_block.execute_callback(Err(err));
                return;
            }
        }

        ckb_metrics::handle().map(|metrics| {
            metrics
                .ckb_chain_lonely_block_ch_len
                .set(self.lonely_block_tx.len() as i64)
        });

        match self.lonely_block_tx.send(lonely_block) {
            Ok(_) => {
                debug!(
                    "processing block: {}-{}, (tip:unverified_tip):({}:{})",
                    block_number,
                    block_hash,
                    self.shared.snapshot().tip_number(),
                    self.shared.get_unverified_tip().number(),
                );
            }
            Err(_) => {
                error!("Failed to notify new block to orphan pool, It seems that the orphan pool has exited.");
            }
        }
    }
}
