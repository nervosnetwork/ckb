use crate::relayer::{Relayer, MAX_RELAY_TXS_NUM_PER_BATCH};
use crate::{Status, StatusCode};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct GetBlockTransactionsProcess<'a> {
    message: packed::GetBlockTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockTransactionsProcess<'a> {
    pub fn new(
        message: packed::GetBlockTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        GetBlockTransactionsProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared = self.relayer.shared();
        {
            let get_block_transactions = self.message;
            if get_block_transactions.indexes().len() > MAX_RELAY_TXS_NUM_PER_BATCH {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "Indexes count({}) > MAX_RELAY_TXS_NUM_PER_BATCH({})",
                    get_block_transactions.indexes().len(),
                    MAX_RELAY_TXS_NUM_PER_BATCH,
                ));
            }
            if get_block_transactions.uncle_indexes().len() > shared.consensus().max_uncles_num() {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "UncleIndexes count({}) > consensus max_uncles_num({})",
                    get_block_transactions.uncle_indexes().len(),
                    shared.consensus().max_uncles_num(),
                ));
            }
        }

        let block_hash = self.message.block_hash().to_entity();
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "get_block_transactions {}",
            block_hash
        );

        if let Some(block) = shared.store().get_block(&block_hash) {
            let transactions = self
                .message
                .indexes()
                .iter()
                .filter_map(|i| {
                    block
                        .transactions()
                        .get(Unpack::<u32>::unpack(&i) as usize)
                        .cloned()
                })
                .collect::<Vec<_>>();

            let uncles = self
                .message
                .uncle_indexes()
                .iter()
                .filter_map(|i| block.uncles().get(Unpack::<u32>::unpack(&i) as usize))
                .collect::<Vec<_>>();

            let content = packed::BlockTransactions::new_builder()
                .block_hash(block_hash)
                .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
                .uncles(uncles.into_iter().map(|uncle| uncle.data()).pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();
            let data = message.as_slice().into();

            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                return StatusCode::Network
                    .with_context(format!("Send BlockTransactions error: {:?}", err));
            }
        }

        Status::ok()
    }
}
