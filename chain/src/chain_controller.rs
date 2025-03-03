//! CKB chain controller.
#![allow(missing_docs)]
#![allow(elided_named_lifetimes)]

use crate::utils::orphan_block_pool::OrphanBlockPool;
use crate::{LonelyBlock, ProcessBlockRequest, RemoteBlock, TruncateRequest, VerifyResult};
use ckb_channel::Sender;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{self, error};
use ckb_store::ChainDB;
use ckb_types::{
    core::{service::Request, BlockView},
    packed::Byte32,
};
use ckb_verification_traits::Switch;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Controller to the chain service.
///
/// The controller is internally reference-counted and can be freely cloned.
///
/// A controller can invoke ChainService methods.
#[cfg_attr(feature = "mock", faux::create)]
#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<ProcessBlockRequest>,
    truncate_sender: Sender<TruncateRequest>,
    orphan_block_broker: Arc<OrphanBlockPool>,

    is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,
}

#[cfg_attr(feature = "mock", faux::methods)]
impl ChainController {
    pub(crate) fn new(
        process_block_sender: Sender<ProcessBlockRequest>,
        truncate_sender: Sender<TruncateRequest>,
        orphan_block_broker: Arc<OrphanBlockPool>,
        is_verifying_unverified_blocks_on_startup: Arc<AtomicBool>,
    ) -> Self {
        ChainController {
            process_block_sender,
            truncate_sender,
            orphan_block_broker,
            is_verifying_unverified_blocks_on_startup,
        }
    }

    pub fn is_verifying_unverified_blocks_on_startup(&self) -> bool {
        self.is_verifying_unverified_blocks_on_startup
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn asynchronous_process_remote_block(&self, remote_block: RemoteBlock) {
        let lonely_block = LonelyBlock {
            block: remote_block.block,
            verify_callback: Some(remote_block.verify_callback),
            switch: None,
        };
        self.asynchronous_process_lonely_block(lonely_block);
    }

    pub fn asynchronous_process_lonely_block(&self, lonely_block: LonelyBlock) {
        if Request::call(&self.process_block_sender, lonely_block).is_none() {
            error!("Chain service has gone")
        }
    }

    /// MinerRpc::submit_block and `ckb import` need this blocking way to process block
    pub fn blocking_process_block(&self, block: Arc<BlockView>) -> VerifyResult {
        self.blocking_process_block_internal(block, None)
    }

    /// `IntegrationTestRpcImpl::process_block_without_verify` need this
    pub fn blocking_process_block_with_switch(
        &self,
        block: Arc<BlockView>,
        switch: Switch,
    ) -> VerifyResult {
        self.blocking_process_block_internal(block, Some(switch))
    }

    fn blocking_process_block_internal(
        &self,
        block: Arc<BlockView>,
        switch: Option<Switch>,
    ) -> VerifyResult {
        let (verify_result_tx, verify_result_rx) = ckb_channel::oneshot::channel::<VerifyResult>();

        let verify_callback = {
            move |result: VerifyResult| {
                if let Err(err) = verify_result_tx.send(result) {
                    error!(
                        "blocking send verify_result failed: {}, this shouldn't happen",
                        err
                    )
                }
            }
        };

        let lonely_block = LonelyBlock {
            block,
            switch,
            verify_callback: Some(Box::new(verify_callback)),
        };

        self.asynchronous_process_lonely_block(lonely_block);
        verify_result_rx.recv().unwrap_or_else(|err| {
            Err(InternalErrorKind::System
                .other(format!("blocking recv verify_result failed: {}", err))
                .into())
        })
    }

    /// Truncate chain to specified target
    ///
    /// Should use for testing only
    pub fn truncate(&self, target_tip_hash: Byte32) -> Result<(), Error> {
        Request::call(&self.truncate_sender, target_tip_hash).unwrap_or_else(|| {
            Err(InternalErrorKind::System
                .other("Chain service has gone")
                .into())
        })
    }

    /// `Relayer::reconstruct_block` need this
    pub fn get_orphan_block(&self, store: &ChainDB, hash: &Byte32) -> Option<Arc<BlockView>> {
        self.orphan_block_broker.get_block(store, hash)
    }

    /// `NetRpcImpl::sync_state` rpc need this
    pub fn orphan_blocks_len(&self) -> usize {
        self.orphan_block_broker.len()
    }
}
