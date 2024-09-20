#![allow(missing_docs)]

//! Bootstrap InitLoadUnverified, PreloadUnverifiedBlock, ChainService and ConsumeUnverified threads.
use crate::chain_service::ChainService;
use crate::init_load_unverified::InitLoadUnverified;
use crate::orphan_broker::OrphanBroker;
use crate::preload_unverified_blocks_channel::PreloadUnverifiedBlocksChannel;
use crate::utils::orphan_block_pool::OrphanBlockPool;
use crate::verify::ConsumeUnverifiedBlocks;
use crate::{chain_controller::ChainController, LonelyBlockHash, UnverifiedBlock};
use ckb_channel::{self as channel, SendError};
use ckb_constant::sync::BLOCK_DOWNLOAD_WINDOW;
use ckb_logger::warn;
use ckb_shared::ChainServicesBuilder;
use ckb_stop_handler::register_thread;
use ckb_types::packed::Byte32;
use dashmap::DashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

const ORPHAN_BLOCK_SIZE: usize = BLOCK_DOWNLOAD_WINDOW as usize;

pub fn start_chain_services(builder: ChainServicesBuilder) -> ChainController {
    let orphan_blocks_broker = Arc::new(OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE));

    let (truncate_block_tx, truncate_block_rx) = channel::bounded(1);

    let (preload_unverified_stop_tx, preload_unverified_stop_rx) = ckb_channel::bounded::<()>(1);

    let (preload_unverified_tx, preload_unverified_rx) =
        channel::bounded::<LonelyBlockHash>(BLOCK_DOWNLOAD_WINDOW as usize * 10);

    let (unverified_queue_stop_tx, unverified_queue_stop_rx) = ckb_channel::bounded::<()>(1);
    let (unverified_block_tx, unverified_block_rx) = channel::bounded::<UnverifiedBlock>(128usize);

    let is_pending_verify: Arc<DashSet<Byte32>> = Arc::new(DashSet::new());

    let consumer_unverified_thread = thread::Builder::new()
        .name("verify_blocks".into())
        .spawn({
            let shared = builder.shared.clone();
            let is_pending_verify = Arc::clone(&is_pending_verify);
            move || {
                let consume_unverified = ConsumeUnverifiedBlocks::new(
                    shared,
                    unverified_block_rx,
                    truncate_block_rx,
                    builder.proposal_table,
                    is_pending_verify,
                    unverified_queue_stop_rx,
                );

                consume_unverified.start();
            }
        })
        .expect("start unverified_queue consumer thread should ok");

    let preload_unverified_block_thread = thread::Builder::new()
        .name("preload_unverified_block".into())
        .spawn({
            let shared = builder.shared.clone();
            move || {
                let preload_unverified_block = PreloadUnverifiedBlocksChannel::new(
                    shared,
                    preload_unverified_rx,
                    unverified_block_tx,
                    preload_unverified_stop_rx,
                );
                preload_unverified_block.start()
            }
        })
        .expect("start preload_unverified_block should ok");

    let (process_block_tx, process_block_rx) = channel::bounded(0);

    let is_verifying_unverified_blocks_on_startup = Arc::new(AtomicBool::new(true));

    let chain_controller = ChainController::new(
        process_block_tx,
        truncate_block_tx,
        Arc::clone(&orphan_blocks_broker),
        Arc::clone(&is_verifying_unverified_blocks_on_startup),
    );

    let init_load_unverified_thread = thread::Builder::new()
        .name("init_load_unverified_blocks".into())
        .spawn({
            let chain_controller = chain_controller.clone();
            let shared = builder.shared.clone();

            move || {
                let init_load_unverified: InitLoadUnverified = InitLoadUnverified::new(
                    shared,
                    chain_controller,
                    is_verifying_unverified_blocks_on_startup,
                );
                init_load_unverified.start();
            }
        })
        .expect("start unverified_queue consumer thread should ok");

    let consume_orphan = OrphanBroker::new(
        builder.shared.clone(),
        orphan_blocks_broker,
        preload_unverified_tx,
        is_pending_verify,
    );

    let chain_service: ChainService =
        ChainService::new(builder.shared, process_block_rx, consume_orphan);
    let chain_service_thread = thread::Builder::new()
        .name("ChainService".into())
        .spawn({
            move || {
                chain_service.start_process_block();

                let _ = init_load_unverified_thread.join();

                if preload_unverified_stop_tx.send(()).is_err(){
                    warn!("trying to notify preload unverified thread to stop, but preload_unverified_stop_tx already closed");
                }
                let _ = preload_unverified_block_thread.join();

                if let Err(SendError(_)) = unverified_queue_stop_tx.send(()) {
                    warn!("trying to notify consume unverified thread to stop, but unverified_queue_stop_tx already closed");
                }
                let _ = consumer_unverified_thread.join();
            }
        })
        .expect("start chain_service thread should ok");
    register_thread("ChainService", chain_service_thread);

    chain_controller
}
