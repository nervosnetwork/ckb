use std::collections::HashSet;

use crate::{
    U256,
    core::{self},
    packed,
    prelude::*,
    utilities::{compact_to_difficulty, merkle_root},
};

impl Difficulty for packed::RawHeader {
    /// Calculates the difficulty from compact target.
    fn difficulty(&self) -> U256 {
        compact_to_difficulty(self.compact_target().into())
    }
}

impl Difficulty for packed::Header {
    /// Calculates the difficulty from compact target.
    fn difficulty(&self) -> U256 {
        self.raw().difficulty()
    }
}

impl ResetBlock for packed::Block {
    /// Recalculates all hashes and merkle roots in the header.
    fn reset_header(self) -> packed::Block {
        let tx_hashes = self.as_reader().calc_tx_hashes();
        let tx_witness_hashes = self.as_reader().calc_tx_witness_hashes();
        self.reset_header_with_hashes(&tx_hashes[..], &tx_witness_hashes[..])
    }

    fn reset_header_with_hashes(
        self,
        tx_hashes: &[packed::Byte32],
        tx_witness_hashes: &[packed::Byte32],
    ) -> packed::Block {
        let raw_transactions_root = merkle_root(tx_hashes);
        let witnesses_root = merkle_root(tx_witness_hashes);
        let transactions_root = merkle_root(&[raw_transactions_root, witnesses_root]);
        let proposals_hash = self.as_reader().calc_proposals_hash();
        let extra_hash = self.as_reader().calc_extra_hash().extra_hash();
        let raw_header = self
            .header()
            .raw()
            .as_builder()
            .transactions_root(transactions_root)
            .proposals_hash(proposals_hash)
            .extra_hash(extra_hash)
            .build();
        let header = self.header().as_builder().raw(raw_header).build();
        if let Some(extension) = self.extension() {
            packed::BlockV1::new_builder()
                .header(header)
                .uncles(self.uncles())
                .transactions(self.transactions())
                .proposals(self.proposals())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            self.as_builder().header(header).build()
        }
    }
}

impl BuildCompactBlock for packed::CompactBlock {
    /// Builds a `CompactBlock` from block and prefilled transactions indexes.
    fn build_from_block(
        block: &core::BlockView,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> packed::CompactBlock {
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
                    .index(transaction_index)
                    .transaction(transaction.data())
                    .build();
                prefilled_transactions.push(prefilled_tx);
            } else {
                short_ids.push(transaction.proposal_short_id());
            }
        }

        if let Some(extension) = block.data().extension() {
            packed::CompactBlockV1::new_builder()
                .header(block.data().header())
                .short_ids(short_ids)
                .prefilled_transactions(prefilled_transactions)
                .uncles(block.uncle_hashes.clone())
                .proposals(block.data().proposals())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::CompactBlock::new_builder()
                .header(block.data().header())
                .short_ids(short_ids)
                .prefilled_transactions(prefilled_transactions)
                .uncles(block.uncle_hashes.clone())
                .proposals(block.data().proposals())
                .build()
        }
    }

    /// Takes proposal short ids for the transactions which are not prefilled.
    fn block_short_ids(&self) -> Vec<Option<packed::ProposalShortId>> {
        let txs_len = self.txs_len();
        let mut block_short_ids: Vec<Option<packed::ProposalShortId>> = Vec::with_capacity(txs_len);
        let prefilled_indexes = self
            .prefilled_transactions()
            .into_iter()
            .map(|tx_index| tx_index.index().into())
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

    /// Collects the short id indexes.
    fn short_id_indexes(&self) -> Vec<usize> {
        let prefilled_indexes_iter = self
            .prefilled_transactions()
            .into_iter()
            .map(|i| i.index().into());

        let prefilled_indexes: HashSet<usize> = prefilled_indexes_iter.collect();

        (0..self.txs_len())
            .filter(|index| !prefilled_indexes.contains(index))
            .collect()
    }
}

impl<'r> CalcExtraHash for packed::BlockReader<'r> {
    /// Calculates the extra hash, which is a combination of the uncles hash and
    /// the extension hash.
    ///
    /// - If there is no extension, extra hash is the same as the uncles hash.
    /// - If there is a extension, then extra hash it the hash of the combination
    ///   of uncles hash and the extension hash.
    fn calc_extra_hash(&self) -> core::ExtraHashView {
        crate::core::ExtraHashView::new(self.calc_uncles_hash(), self.calc_extension_hash())
    }
}

impl CalcExtraHash for packed::Block {
    /// Calls [`BlockReader.calc_extra_hash()`](struct.BlockReader.html#method.calc_extra_hash)
    /// for [`self.as_reader()`](struct.Block.html#method.as_reader).
    fn calc_extra_hash(&self) -> core::ExtraHashView {
        self.as_reader().calc_extra_hash()
    }
}

#[cfg(test)]
mod test {
    use crate::{h256, packed, prelude::*};
    #[test]
    fn empty_extra_hash() {
        let block = packed::Block::new_builder().build();
        let expect = h256!("0x0");
        assert_eq!(block.calc_extra_hash().extra_hash(), expect.into());
    }
}
