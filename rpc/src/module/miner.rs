use crate::error::RPCError;
use ckb_chain::chain::ChainController;
use ckb_jsonrpc_types::{Block, BlockTemplate, Uint64, Version};
use ckb_logger::{debug, error};
use ckb_network::{NetworkController, SupportProtocols};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_types::{core, packed, prelude::*, H256};
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use faketime::unix_time_as_millis;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;

#[rpc(server)]
pub trait MinerRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_block_template")]
    fn get_block_template(
        &self,
        bytes_limit: Option<Uint64>,
        proposals_limit: Option<Uint64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "transactions":[], "proposals":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_block")]
    fn submit_block(&self, _work_id: String, _data: Block) -> Result<H256>;
}

pub(crate) struct MinerRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl MinerRpc for MinerRpcImpl {
    fn get_block_template(
        &self,
        bytes_limit: Option<Uint64>,
        proposals_limit: Option<Uint64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate> {
        let bytes_limit = match bytes_limit {
            Some(b) => Some(b.into()),
            None => None,
        };

        let proposals_limit = match proposals_limit {
            Some(b) => Some(b.into()),
            None => None,
        };

        let tx_pool = self.shared.tx_pool_controller();

        tx_pool
            .get_block_template(bytes_limit, proposals_limit, max_version.map(Into::into))
            .map_err(|err| {
                error!("send get_block_template request error {}", err);
                RPCError::ckb_internal_error(err)
            })?
            .map_err(|err| {
                error!("get_block_template result error {}", err);
                RPCError::ckb_internal_error(err)
            })
    }

    fn submit_block(&self, work_id: String, data: Block) -> Result<H256> {
        // TODO: this API is intended to be used in a trusted environment, thus it should pass the
        // verifier. We use sentry to capture errors found here to discovery issues early, which
        // should be removed later.
        let _scope_guard = sentry::Hub::current().push_scope();
        sentry::configure_scope(|scope| scope.set_extra("work_id", work_id.clone().into()));

        debug!("[{}] submit block", work_id);
        let block: packed::Block = data.into();
        let block: Arc<core::BlockView> = Arc::new(block.into_view());
        let header = block.header();

        // Verify header
        let snapshot: &Snapshot = &self.shared.snapshot();
        let resolver = HeaderResolverWrapper::new(&header, snapshot);
        HeaderVerifier::new(snapshot, &self.shared.consensus())
            .verify(&resolver)
            .map_err(|err| handle_submit_error(&work_id, &err))?;

        // Verify and insert block
        let is_new = self
            .chain
            .process_block(Arc::clone(&block))
            .map_err(|err| handle_submit_error(&work_id, &err))?;

        // Announce only new block
        if is_new {
            debug!(
                "[block_relay] announce new block {} {} {}",
                header.number(),
                header.hash(),
                unix_time_as_millis()
            );
            let content = packed::CompactBlock::build_from_block(&block, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(content).build();
            if let Err(err) = self
                .network_controller
                .quick_broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
            {
                error!("Broadcast new block failed: {:?}", err);
            }
        }

        Ok(header.hash().unpack())
    }
}

fn handle_submit_error<E: Debug + ToString>(work_id: &str, err: &E) -> Error {
    error!("[{}] submit_block error: {:?}", work_id, err);
    capture_submit_error(err);
    RPCError::custom(RPCError::Invalid, err.to_string())
}

fn capture_submit_error<D: Debug>(err: &D) {
    use sentry::{capture_message, with_scope, Level};
    with_scope(
        |scope| scope.set_fingerprint(Some(&["ckb-rpc", "miner", "submit_block"])),
        || capture_message(&format!("submit_block {:?}", err), Level::Error),
    );
}
