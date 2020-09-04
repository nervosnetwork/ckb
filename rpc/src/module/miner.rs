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

/// RPC Module Miner for miners.
///
/// A miner gets a template from CKB, optionally selects transactions, resolves the PoW puzzle, and
/// submits the found new block.
#[rpc(server)]
pub trait MinerRpc {
    /// Returns block template for miners.
    ///
    /// Miners can assemble the new block from the template. The RPC is designed to allow miners
    /// removing transactions and adding new transactions to the block.
    ///
    /// ## Params
    ///
    /// * `bytes_limit` - the max serialization size in bytes of the block.
    ///     (**Optional:** the default is the consensus limit.)
    /// * `proposals_limit` - the max count of proposals.
    ///     (**Optional:** the default is the consensus limit.)
    /// * `max_version` - the max block version.
    ///     (**Optional:** the default is one configured in the current client version.)
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_template",
    ///   "params": [
    ///     null,
    ///     null,
    ///     null
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "bytes_limit": "0x22d387",
    ///     "cellbase": {
    ///       "cycles": null,
    ///       "data": {
    ///         "cell_deps": [],
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x1"
    ///           }
    ///         ],
    ///         "outputs": [
    ///           {
    ///             "capacity": "0x1d1a94a200",
    ///             "lock": {
    ///               "args": [
    ///                 "0xb2e61ff569acf041b3c2c17724e2379c581eeac3"
    ///               ],
    ///               "code_hash": "0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df2",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           {
    ///             "data": [
    ///               "0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df201",
    ///               "0x"
    ///             ]
    ///           }
    ///         ]
    ///       },
    ///       "hash": "0x076049e2cc6b9f1ed4bb27b2337c55071dabfaf0183b1b17a4965bd0372d8dec"
    ///     },
    ///     "compact_target": "0x100",
    ///     "current_time": "0x16d6269e84f",
    ///     "cycles_limit": "0x2540be400",
    ///     "dao": "0x004fb9e277860700b2f80165348723003d1862ec960000000028eb3d7e7a0100",
    ///     "epoch": "0x3e80001000000",
    ///     "number": "0x1",
    ///     "parent_hash": "0xd5c495b7dd4d9d066a6a4d4356bc31955ad3199e0d856f34cfbe159c46ee335b",
    ///     "proposals": [],
    ///     "transactions": [],
    ///     "uncles": [],
    ///     "uncles_count_limit": "0x2",
    ///     "version": "0x0",
    ///     "work_id": "0x0"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_block_template")]
    fn get_block_template(
        &self,
        bytes_limit: Option<Uint64>,
        proposals_limit: Option<Uint64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate>;

    /// Submit new block to network
    ///
    /// ## Params
    ///
    /// * `work_id` - The same work ID returned from [`get_block_template`](#tymethod.get_block_template).
    /// * `block` - The assembed block from the block template and which PoW puzzle has been resolved.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "submit_block",
    ///   "params": [
    ///     "example",
    ///     {
    ///       "header": {
    ///         "compact_target": "0x1e083126",
    ///         "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///         "epoch": "0x7080018000001",
    ///         "nonce": "0x0",
    ///         "number": "0x400",
    ///         "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///         "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///         "timestamp": "0x5cd2b117",
    ///         "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///         "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///         "version": "0x0"
    ///       },
    ///       "proposals": [],
    ///       "transactions": [
    ///         {
    ///           "cell_deps": [],
    ///           "header_deps": [],
    ///           "inputs": [
    ///             {
    ///               "previous_output": {
    ///                 "index": "0xffffffff",
    ///                 "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///               },
    ///               "since": "0x400"
    ///             }
    ///           ],
    ///           "outputs": [
    ///             {
    ///               "capacity": "0x18e64b61cf",
    ///               "lock": {
    ///                 "args": "0x",
    ///                 "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///                 "hash_type": "data"
    ///               },
    ///               "type": null
    ///             }
    ///           ],
    ///           "outputs_data": [
    ///             "0x"
    ///           ],
    ///           "version": "0x0",
    ///           "witnesses": [
    ///             "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
    ///           ]
    ///         }
    ///       ],
    ///       "uncles": []
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    /// }
    /// ```
    #[rpc(name = "submit_block")]
    fn submit_block(&self, work_id: String, block: Block) -> Result<H256>;
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
                RPCError::from_failure_error(err)
            })
    }

    fn submit_block(&self, work_id: String, block: Block) -> Result<H256> {
        debug!("[{}] submit block", work_id);
        let block: packed::Block = block.into();
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

fn handle_submit_error<E: std::fmt::Display + Debug>(work_id: &str, err: &E) -> Error {
    error!("[{}] submit_block error: {:?}", work_id, err);
    RPCError::custom_with_error(RPCError::Invalid, err)
}
