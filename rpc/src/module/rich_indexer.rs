use crate::error::RPCError;
use async_trait::async_trait;
use ckb_jsonrpc_types::IndexerTip;
use ckb_rich_indexer::AsyncRichIndexerHandle;
use jsonrpc_core::Result;
use jsonrpc_utils::rpc;

/// RPC Module Indexer.
#[rpc]
#[async_trait]
pub trait RichIndexerRpc {
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
    ///     "method": "get_rich_indexer_tip"
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
    #[rpc(name = "get_rich_indexer_tip")]
    async fn get_rich_indexer_tip(&self) -> Result<Option<IndexerTip>>;
}

#[derive(Clone)]
pub(crate) struct RichIndexerRpcImpl {
    pub(crate) handle: AsyncRichIndexerHandle,
}

impl RichIndexerRpcImpl {
    pub fn new(handle: AsyncRichIndexerHandle) -> Self {
        RichIndexerRpcImpl { handle }
    }
}

#[async_trait]
impl RichIndexerRpc for RichIndexerRpcImpl {
    async fn get_rich_indexer_tip(&self) -> Result<Option<IndexerTip>> {
        self.handle
            .query_indexer_tip()
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }
}
