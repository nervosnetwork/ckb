use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

/// RPC Module System for checking system information.
#[rpc(server)]
pub trait SystemRpc {
    /// Returns `true` if all CKB services (database, network, chain and so on) are ready to work.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "is_ready",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": true
    /// }
    /// ```
    #[rpc(name = "is_ready")]
    fn is_ready(&self) -> Result<bool>;
}

// Do NOT add any references of runtime states into this struct.
// This struct should be always available and the result should be conclusive.
pub(crate) struct SystemRpcImpl {
    is_ready: bool,
}

impl SystemRpcImpl {
    pub fn new(is_ready: bool) -> Self {
        SystemRpcImpl { is_ready }
    }
}

impl SystemRpc for SystemRpcImpl {
    fn is_ready(&self) -> Result<bool> {
        Ok(self.is_ready)
    }
}
