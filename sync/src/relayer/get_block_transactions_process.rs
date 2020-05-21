use crate::relayer::{Relayer, MAX_RELAY_TXS_NUM_PER_BATCH};
use crate::utils::send_blocktransactions;
use crate::{Status, StatusCode};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct GetBlockTransactionsProcess<'a> {
    get_block_transactions: packed::GetBlockTransactions,
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
        let get_block_transactions = message.to_entity();
        GetBlockTransactionsProcess {
            get_block_transactions,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        {
            fail::fail_point!("recv_getblocktransactions", |_| {
                let block_hash = self.get_block_transactions.block_hash();
                let indexes_length = self.get_block_transactions.indexes().len();
                let uncle_indexes_length = self.get_block_transactions.uncle_indexes().len();
                ckb_logger::debug!(
                    "recv_getblocktransactions(block_hash: {:?}, indexes_len={}, uncle_indexes_len={}) from {}",
                    block_hash, indexes_length, uncle_indexes_length, self.peer
                );
                Status::ignored()
            })
        }

        let shared = self.relayer.shared();
        {
            let get_block_transactions = &self.get_block_transactions;
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

        let block_hash = self.get_block_transactions.block_hash();
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "get_block_transactions {}",
            block_hash
        );

        if let Some(block) = shared.store().get_block(&block_hash) {
            let transactions = self
                .get_block_transactions
                .indexes()
                .into_iter()
                .filter_map(|i| {
                    block
                        .transactions()
                        .get(Unpack::<u32>::unpack(&i) as usize)
                        .cloned()
                })
                .collect::<Vec<_>>();

            let uncles = self
                .get_block_transactions
                .uncle_indexes()
                .into_iter()
                .filter_map(|i| block.uncles().get(Unpack::<u32>::unpack(&i) as usize))
                .collect::<Vec<_>>();

            if let Err(err) = send_blocktransactions(
                self.nc.as_ref(),
                self.peer,
                block_hash,
                transactions.into_iter().map(|tx| tx.data()).collect(),
                uncles.into_iter().map(|uncle| uncle.data()).collect(),
            ) {
                return StatusCode::Network
                    .with_context(format!("send_blocktransactions error: {:?}", err));
            }
        }

        Status::ok()
    }
}
