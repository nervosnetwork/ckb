use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::Transaction;
use numext_fixed_hash::H256;
use std::convert::TryInto;

#[rpc]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    #[rpc(name = "enqueue_test_transaction")]
    fn enqueue_test_transaction(&self, _tx: Transaction) -> Result<H256>;
}

pub(crate) struct IntegrationTestRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> IntegrationTestRpc for IntegrationTestRpcImpl<CS> {
    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network_controller.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }

    fn enqueue_test_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        let mut chain_state = self.shared.chain_state().lock();
        let tx_hash = tx.hash().clone();
        chain_state.mut_tx_pool().enqueue_tx(None, tx);
        Ok(tx_hash)
    }
}
