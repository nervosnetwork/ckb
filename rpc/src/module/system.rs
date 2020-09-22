use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

#[rpc(server)]
pub trait SystemRpc {
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
