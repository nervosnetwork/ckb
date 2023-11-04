use crate::error::RPCError;
use ckb_indexer_r::IndexerRHandle;
use ckb_jsonrpc_types::IndexerTip;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

/// RPC Module Indexer.
#[rpc(server)]
pub trait IndexerRRpc {
    /// Returns the indexed tip
    ///
    /// ## Returns
    ///   * block_hash - indexed tip block hash
    ///   * block_number - indexed tip block number
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_indexer_r_tip"
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "block_hash": "0x4959d6e764a2edc6038dbf03d61ebcc99371115627b186fdcccb2161fbd26edc",
    ///     "block_number": "0x5b513e"
    ///   },
    ///   "id": 2
    /// }
    /// ```
    #[rpc(name = "get_indexer_r_tip")]
    fn get_indexer_r_tip(&self) -> Result<Option<IndexerTip>>;
}

pub(crate) struct IndexerRRpcImpl {
    pub(crate) handle: IndexerRHandle,
}

impl IndexerRRpcImpl {
    pub fn new(handle: IndexerRHandle) -> Self {
        IndexerRRpcImpl { handle }
    }
}

impl IndexerRRpc for IndexerRRpcImpl {
    fn get_indexer_r_tip(&self) -> Result<Option<IndexerTip>> {
        self.handle
            .get_indexer_tip()
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }
}
