use crate::error::RPCError;
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain::{chain::ChainController, switch::Switch};
use ckb_jsonrpc_types::{Block, Cycle, JsonBytes, Script, Transaction};
use ckb_logger::error;
use ckb_network::{NetworkController, SupportProtocols};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{core, packed, prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;
use std::sync::Arc;

#[rpc(server)]
#[doc(hidden)]
pub trait IntegrationTestRpc {
    #[rpc(name = "process_block_without_verify")]
    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>>;

    #[rpc(name = "truncate")]
    fn truncate(&self, target_tip_hash: H256) -> Result<()>;

    #[rpc(name = "generate_block")]
    fn generate_block(
        &self,
        block_assembler_script: Option<Script>,
        block_assembler_message: Option<JsonBytes>,
    ) -> Result<H256>;

    #[rpc(name = "broadcast_transaction")]
    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256>;
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>> {
        let block: packed::Block = data.into();
        let block: Arc<core::BlockView> = Arc::new(block.into_view());
        let ret = self
            .chain
            .internal_process_block(Arc::clone(&block), Switch::DISABLE_ALL);

        if broadcast {
            let content = packed::CompactBlock::build_from_block(&block, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(content).build();
            if let Err(err) = self
                .network_controller
                .quick_broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
            {
                error!("Broadcast new block failed: {:?}", err);
            }
        }
        if ret.is_ok() {
            Ok(Some(block.hash().unpack()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }

    fn truncate(&self, target_tip_hash: H256) -> Result<()> {
        let header = {
            let snapshot = self.shared.snapshot();
            let header = snapshot
                .get_block_header(&target_tip_hash.pack())
                .ok_or_else(|| {
                    RPCError::custom(RPCError::Invalid, "block not found".to_string())
                })?;
            if !snapshot.is_main_chain(&header.hash()) {
                return Err(RPCError::custom(
                    RPCError::Invalid,
                    "block not on main chain".to_string(),
                ));
            }
            header
        };

        // Truncate the chain and database
        self.chain
            .truncate(header.hash())
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        // Clear the tx_pool
        let new_snapshot = Arc::clone(&self.shared.snapshot());
        let tx_pool = self.shared.tx_pool_controller();
        tx_pool
            .clear_pool(new_snapshot)
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        Ok(())
    }

    fn generate_block(
        &self,
        block_assembler_script: Option<Script>,
        block_assembler_message: Option<JsonBytes>,
    ) -> Result<H256> {
        let tx_pool = self.shared.tx_pool_controller();
        let block_assembler_config = block_assembler_script.map(|script| BlockAssemblerConfig {
            code_hash: script.code_hash,
            hash_type: script.hash_type,
            args: script.args,
            message: block_assembler_message.unwrap_or_default(),
        });
        let block_template = tx_pool
            .get_block_template_with_block_assembler_config(
                None,
                None,
                None,
                block_assembler_config,
            )
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?
            .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;

        let block: packed::Block = block_template.into();
        let block_view = Arc::new(block.into_view());
        let content = packed::CompactBlock::build_from_block(&block_view, &HashSet::new());
        let message = packed::RelayMessage::new_builder().set(content).build();

        // insert block to chain
        self.chain
            .process_block(Arc::clone(&block_view))
            .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;

        // announce new block
        if let Err(err) = self
            .network_controller
            .quick_broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
        {
            error!("Broadcast new block failed: {:?}", err);
        }

        Ok(block_view.header().hash().unpack())
    }

    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256> {
        let tx: packed::Transaction = transaction.into();
        let hash = tx.calc_tx_hash();
        let relay_tx = packed::RelayTransaction::new_builder()
            .cycles(cycles.value().pack())
            .transaction(tx)
            .build();
        let relay_txs = packed::RelayTransactions::new_builder()
            .transactions(vec![relay_tx].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(relay_txs).build();

        if let Err(err) = self
            .network_controller
            .broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
        {
            error!("Broadcast transaction failed: {:?}", err);
            Err(RPCError::custom_with_error(
                RPCError::P2PFailedToBroadcast,
                err,
            ))
        } else {
            Ok(hash.unpack())
        }
    }
}
