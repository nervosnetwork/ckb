use std::collections::HashSet;
use std::convert::TryFrom;

use ckb_merkle_tree::merkle_root;

use crate::{
    core::{self, BlockNumber, ScriptHashType},
    packed,
    prelude::*,
    H256,
};

impl packed::ProposalShortId {
    pub fn from_tx_hash(h: &H256) -> Self {
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&h.as_bytes()[..10]);
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
    pub fn new(tx_hash: H256, index: u32) -> Self {
        packed::OutPoint::new_builder()
            .tx_hash(tx_hash.pack())
            .index(index.pack())
            .build()
    }

    pub fn null() -> Self {
        packed::OutPoint::new_builder()
            .index(u32::max_value().pack())
            .build()
    }

    pub fn is_null(&self) -> bool {
        Unpack::<H256>::unpack(&self.tx_hash()).is_zero()
            && Unpack::<u32>::unpack(&self.index()) == u32::max_value()
    }
}

impl packed::CellDep {
    pub fn new(out_point: packed::OutPoint, is_dep_group: bool) -> packed::CellDep {
        packed::CellDep::new_builder()
            .out_point(out_point)
            .is_dep_group(is_dep_group.pack())
            .build()
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
    pub fn into_witness(self) -> packed::Witness {
        let mut code_hash_and_hash_type = self.code_hash().as_slice().to_vec();
        let hash_type: ScriptHashType = self.hash_type().unpack();
        code_hash_and_hash_type.push(hash_type as u8);
        packed::Witness::new_builder()
            .push((&code_hash_and_hash_type[..]).pack())
            .extend(self.args().into_iter())
            .build()
    }

    pub fn from_witness(witness: packed::Witness) -> Option<Self> {
        if !witness.is_empty() {
            let mut args = witness.into_iter();
            let first = args.next().should_be_ok();
            let len = first.raw_data().len();
            if len == 33 {
                let code_hash = H256::from_slice(&first.raw_data()[..(len - 1)])
                    .expect("impossible: fail to create H256 from slice");
                if let Ok(hash_type) = ScriptHashType::try_from(first.raw_data()[len - 1]) {
                    let args = packed::BytesVec::new_builder().extend(args).build();
                    let script = packed::Script::new_builder()
                        .code_hash(code_hash.pack())
                        .args(args)
                        .hash_type(hash_type.pack())
                        .build();
                    Some(script)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl packed::Transaction {
    pub fn is_cellbase(&self) -> bool {
        let raw_tx = self.raw();
        raw_tx.inputs().len() == 1
            && raw_tx.outputs().len() == 1
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
        tx_hashes: &[H256],
        tx_witness_hashes: &[H256],
    ) -> packed::Block {
        let transactions_root = merkle_root(tx_hashes);
        let witnesses_root = merkle_root(tx_witness_hashes);
        let proposals_hash = self.as_reader().calc_proposals_hash();
        let uncles_hash = self.as_reader().calc_uncles_hash();
        let uncles_count = self.as_reader().uncles().len() as u32;
        let raw_header = self
            .header()
            .raw()
            .as_builder()
            .transactions_root(transactions_root.pack())
            .witnesses_root(witnesses_root.pack())
            .proposals_hash(proposals_hash.pack())
            .uncles_hash(uncles_hash.pack())
            .uncles_count(uncles_count.pack())
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
            .uncles(block.data().uncles())
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
