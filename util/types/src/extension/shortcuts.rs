use std::collections::HashSet;

use crate::{
    core::{self, BlockNumber},
    packed,
    prelude::*,
    utilities::{compact_to_difficulty, merkle_root},
    U256,
};

impl packed::Byte32 {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn max_value() -> Self {
        [u8::max_value(); 32].pack()
    }

    pub fn is_zero(&self) -> bool {
        self.as_slice().iter().all(|x| *x == 0)
    }

    pub fn new(v: [u8; 32]) -> Self {
        v.pack()
    }
}

impl packed::ProposalShortId {
    pub fn from_tx_hash(h: &packed::Byte32) -> Self {
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&h.as_slice()[..10]);
        inner.pack()
    }

    pub fn zero() -> Self {
        Self::default()
    }

    pub fn new(v: [u8; 10]) -> Self {
        v.pack()
    }
}

impl packed::OutPoint {
    pub fn new(tx_hash: packed::Byte32, index: u32) -> Self {
        packed::OutPoint::new_builder()
            .tx_hash(tx_hash)
            .index(index.pack())
            .build()
    }

    pub fn null() -> Self {
        packed::OutPoint::new_builder()
            .index(u32::max_value().pack())
            .build()
    }

    pub fn is_null(&self) -> bool {
        self.tx_hash().is_zero() && Unpack::<u32>::unpack(&self.index()) == u32::max_value()
    }
}

impl packed::CellInput {
    pub fn new(previous_output: packed::OutPoint, block_number: BlockNumber) -> Self {
        packed::CellInput::new_builder()
            .since(block_number.pack())
            .previous_output(previous_output)
            .build()
    }
    pub fn new_cellbase_input(block_number: BlockNumber) -> Self {
        Self::new(packed::OutPoint::null(), block_number)
    }
}

impl packed::Script {
    pub fn into_witness(self) -> packed::Bytes {
        packed::CellbaseWitness::new_builder()
            .lock(self)
            .build()
            .as_bytes()
            .pack()
    }

    pub fn from_witness(witness: packed::Bytes) -> Option<Self> {
        packed::CellbaseWitness::from_slice(&witness.raw_data())
            .map(|cellbase_witness| cellbase_witness.lock())
            .ok()
    }

    pub fn is_hash_type_type(&self) -> bool {
        Into::<u8>::into(self.hash_type()) == Into::<u8>::into(core::ScriptHashType::Type)
    }
}

impl packed::Transaction {
    pub fn is_cellbase(&self) -> bool {
        let raw_tx = self.raw();
        raw_tx.inputs().len() == 1
            && self.witnesses().len() == 1
            && raw_tx
                .inputs()
                .get(0)
                .should_be_ok()
                .previous_output()
                .is_null()
    }

    pub fn proposal_short_id(&self) -> packed::ProposalShortId {
        packed::ProposalShortId::from_tx_hash(&self.calc_tx_hash())
    }
}

impl packed::RawHeader {
    pub fn difficulty(&self) -> U256 {
        compact_to_difficulty(self.compact_target().unpack())
    }
}

impl packed::Header {
    pub fn difficulty(&self) -> U256 {
        self.raw().difficulty()
    }
}

impl packed::Block {
    pub fn as_uncle(&self) -> packed::UncleBlock {
        packed::UncleBlock::new_builder()
            .header(self.header())
            .proposals(self.proposals())
            .build()
    }

    pub fn reset_header(self) -> packed::Block {
        let tx_hashes = self.as_reader().calc_tx_hashes();
        let tx_witness_hashes = self.as_reader().calc_tx_witness_hashes();
        self.reset_header_with_hashes(&tx_hashes[..], &tx_witness_hashes[..])
    }

    pub(crate) fn reset_header_with_hashes(
        self,
        tx_hashes: &[packed::Byte32],
        tx_witness_hashes: &[packed::Byte32],
    ) -> packed::Block {
        let raw_transactions_root = merkle_root(tx_hashes);
        let witnesses_root = merkle_root(tx_witness_hashes);
        let transactions_root = merkle_root(&[raw_transactions_root, witnesses_root]);
        let proposals_hash = self.as_reader().calc_proposals_hash();
        let uncles_hash = self.as_reader().calc_uncles_hash();
        let raw_header = self
            .header()
            .raw()
            .as_builder()
            .transactions_root(transactions_root)
            .proposals_hash(proposals_hash)
            .uncles_hash(uncles_hash)
            .build();
        let header = self.header().as_builder().raw(raw_header).build();
        self.as_builder().header(header).build()
    }
}

impl packed::CompactBlock {
    pub fn build_from_block(
        block: &core::BlockView,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> Self {
        // always prefill cellbase
        let prefilled_transactions_len = prefilled_transactions_indexes.len() + 1;
        let mut short_ids: Vec<packed::ProposalShortId> = Vec::with_capacity(
            block
                .data()
                .transactions()
                .len()
                .saturating_sub(prefilled_transactions_len),
        );
        let mut prefilled_transactions = Vec::with_capacity(prefilled_transactions_len);

        for (transaction_index, transaction) in block.transactions().into_iter().enumerate() {
            if prefilled_transactions_indexes.contains(&transaction_index)
                || transaction.is_cellbase()
            {
                let prefilled_tx = packed::IndexTransaction::new_builder()
                    .index((transaction_index as u32).pack())
                    .transaction(transaction.data())
                    .build();
                prefilled_transactions.push(prefilled_tx);
            } else {
                short_ids.push(transaction.proposal_short_id());
            }
        }

        packed::CompactBlock::new_builder()
            .header(block.data().header())
            .short_ids(short_ids.pack())
            .prefilled_transactions(prefilled_transactions.pack())
            .uncles(block.uncle_hashes.clone())
            .proposals(block.data().proposals())
            .build()
    }

    pub fn block_short_ids(&self) -> Vec<Option<packed::ProposalShortId>> {
        let txs_len = self.txs_len();
        let mut block_short_ids: Vec<Option<packed::ProposalShortId>> = Vec::with_capacity(txs_len);
        let prefilled_indexes = self
            .prefilled_transactions()
            .into_iter()
            .map(|tx_index| tx_index.index().unpack())
            .collect::<HashSet<usize>>();

        let mut index = 0;
        for i in 0..txs_len {
            if prefilled_indexes.contains(&i) {
                block_short_ids.push(None);
            } else {
                block_short_ids.push(self.short_ids().get(index));
                index += 1;
            }
        }
        block_short_ids
    }

    pub fn txs_len(&self) -> usize {
        self.prefilled_transactions().len() + self.short_ids().len()
    }

    pub fn prefilled_indexes_iter(&self) -> impl Iterator<Item = usize> {
        self.prefilled_transactions()
            .into_iter()
            .map(|i| i.index().unpack())
    }

    pub fn short_id_indexes(&self) -> Vec<usize> {
        let prefilled_indexes: HashSet<usize> = self.prefilled_indexes_iter().collect();

        (0..self.txs_len())
            .filter(|index| !prefilled_indexes.contains(&index))
            .collect()
    }
}
