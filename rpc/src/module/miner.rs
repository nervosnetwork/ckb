use ckb_chain::chain::ChainController;
use ckb_jsonrpc_types::{Block, BlockTemplate, Unsigned, Version};
use ckb_logger::{debug, error};
use ckb_network::NetworkController;
use ckb_shared::{shared::Shared, Snapshot};
use ckb_sync::NetworkProtocol;
use ckb_traits::ChainProvider;
use ckb_types::{core, packed, prelude::*, H256};
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use faketime::unix_time_as_millis;
use futures::future::Future;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
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

pub(crate) struct MinerRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl MinerRpc for MinerRpcImpl {
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

        let tx_pool = self.shared.tx_pool_controller();
        tx_pool
            .get_block_template(bytes_limit, proposals_limit, max_version.map(|v| v.0))
            .unwrap()
            .wait()
            .unwrap()
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
        let block: packed::Block = data.into();
        let block: Arc<core::BlockView> = Arc::new(block.into_view());
        let header = block.header();
        let resolver =
            HeaderResolverWrapper::new(&header, self.shared.store(), self.shared.consensus());
        let header_verify_ret = {
            let snapshot: &Snapshot = &self.shared.snapshot();
            let header_verifier =
                HeaderVerifier::new(snapshot, Arc::clone(&self.shared.consensus().pow_engine()));
            header_verifier.verify(&resolver)
        };
        if header_verify_ret.is_ok() {
            let ret = self.chain.process_block(Arc::clone(&block), true);
            if ret.is_ok() {
                debug!(
                    "[block_relay] announce new block {} {} {}",
                    block.header().number(),
                    block.header().hash(),
                    unix_time_as_millis()
                );
                // announce new block

                let content = packed::CompactBlock::build_from_block(&block, &HashSet::new());
                let message = packed::RelayMessage::new_builder().set(content).build();
                let data = message.as_slice().into();
                if let Err(err) = self
                    .network_controller
                    .quick_broadcast(NetworkProtocol::RELAY.into(), data)
                {
                    error!("Broadcast block failed: {:?}", err);
                }
                Ok(Some(block.header().hash().unpack()))
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
