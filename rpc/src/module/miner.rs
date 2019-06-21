use ckb_chain::chain::ChainController;
use ckb_core::block::Block as CoreBlock;
use ckb_jsonrpc_types::{Block, BlockTemplate, Unsigned, Version};
use ckb_logger::{debug, error};
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkController;
use ckb_protocol::RelayMessage;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_sync::NetworkProtocol;
use ckb_traits::ChainProvider;
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;

#[rpc]
pub trait MinerRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_block_template")]
    fn get_block_template(
        &self,
        bytes_limit: Option<Unsigned>,
        proposals_limit: Option<Unsigned>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "transactions":[], "proposals":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_block")]
    fn submit_block(&self, _work_id: String, _data: Block) -> Result<Option<H256>>;
}

pub(crate) struct MinerRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
    pub block_assembler: BlockAssemblerController,
    pub chain: ChainController,
}

impl<CS: ChainStore + 'static> MinerRpc for MinerRpcImpl<CS> {
    fn get_block_template(
        &self,
        bytes_limit: Option<Unsigned>,
        proposals_limit: Option<Unsigned>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate> {
        let bytes_limit = match bytes_limit {
            Some(b) => Some(b.0),
            None => None,
        };

        let proposals_limit = match proposals_limit {
            Some(b) => Some(b.0),
            None => None,
        };

        self.block_assembler
            .get_block_template(bytes_limit, proposals_limit, max_version.map(|v| v.0))
            .map_err(|err| {
                error!("get_block_template error {}", err);
                Error::internal_error()
            })
    }

    fn submit_block(&self, work_id: String, data: Block) -> Result<Option<H256>> {
        // TODO: this API is intended to be used in a trusted environment, thus it should pass the
        // verifier. We use sentry to capture errors found here to discovery issues early, which
        // should be removed later.
        let _scope_guard = sentry::Hub::current().push_scope();
        sentry::configure_scope(|scope| scope.set_extra("work_id", work_id.clone().into()));

        debug!("[{}] submit block", work_id);
        let block: Arc<CoreBlock> = Arc::new(data.into());
        let resolver = HeaderResolverWrapper::new(block.header(), self.shared.clone());
        let header_verify_ret = {
            let chain_state = self.shared.lock_chain_state();
            let header_verifier = HeaderVerifier::new(
                &*chain_state,
                Arc::clone(&self.shared.consensus().pow_engine()),
            );
            header_verifier.verify(&resolver)
        };
        if header_verify_ret.is_ok() {
            let ret = self.chain.process_block(Arc::clone(&block), true);
            if ret.is_ok() {
                debug!(
                    "[block_relay] announce new block {} {:x} {}",
                    block.header().number(),
                    block.header().hash(),
                    unix_time_as_millis()
                );
                // announce new block

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                let data = fbb.finished_data().into();
                if let Err(err) = self
                    .network_controller
                    .quick_broadcast(NetworkProtocol::RELAY.into(), data)
                {
                    error!("Broadcast block failed: {:?}", err);
                }
                Ok(Some(block.header().hash().to_owned()))
            } else {
                error!("[{}] submit_block process_block {:?}", work_id, ret);
                use sentry::{capture_message, with_scope, Level};
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-rpc", "miner", "submit_block"])),
                    || {
                        capture_message(
                            &format!("submit_block process_block {:?}", ret),
                            Level::Error,
                        )
                    },
                );
                Ok(None)
            }
        } else {
            error!(
                "[{}] submit_block header verifier {:?}",
                work_id, header_verify_ret
            );
            Ok(None)
        }
    }
}
