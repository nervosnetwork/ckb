use flatbuffers::{FlatBufferBuilder, WIPOffset};
use numext_fixed_hash::H256;

use ckb_core::{
    block::Block,
    extras::{EpochExt, TransactionInfo},
    header::Header,
    transaction::{ProposalShortId, Transaction},
    uncle::UncleBlock,
    Capacity,
};

use crate::{self as protos, CanBuild};

impl<'a> CanBuild<'a> for protos::StoredBlockCache<'a> {
    type Input = Block;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Self::Input,
    ) -> WIPOffset<protos::StoredBlockCache<'b>> {
        let header_hash = block.header().hash().into();
        let mut uncle_hashes: Vec<protos::Bytes32> = Vec::with_capacity(block.uncles().len());
        for uncle in block.uncles() {
            uncle_hashes.push(uncle.hash().into());
        }
        let mut tx_hashes: Vec<protos::Bytes32> = Vec::with_capacity(block.transactions().len());
        let mut tx_witness_hashes: Vec<protos::Bytes32> =
            Vec::with_capacity(block.transactions().len());
        for tx in block.transactions() {
            tx_hashes.push(tx.hash().into());
            tx_witness_hashes.push(tx.witness_hash().into());
        }

        let uncle_hashes = fbb.create_vector(&uncle_hashes);
        let tx_hashes = fbb.create_vector(&tx_hashes);
        let tx_witness_hashes = fbb.create_vector(&tx_witness_hashes);

        let mut builder = protos::StoredBlockCacheBuilder::new(fbb);
        builder.add_header_hash(&header_hash);
        builder.add_uncle_hashes(uncle_hashes);
        builder.add_tx_hashes(tx_hashes);
        builder.add_tx_witness_hashes(tx_witness_hashes);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredBlock<'a> {
    type Input = Block;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Self::Input,
    ) -> WIPOffset<protos::StoredBlock<'b>> {
        let data = protos::Block::build(fbb, block);
        let cache = protos::StoredBlockCache::build(fbb, block);
        let mut builder = protos::StoredBlockBuilder::new(fbb);
        builder.add_data(data);
        builder.add_cache(cache);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredBlockBodyCache<'a> {
    type Input = [Transaction];
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        transactions: &Self::Input,
    ) -> WIPOffset<protos::StoredBlockBodyCache<'b>> {
        let mut tx_hashes: Vec<protos::Bytes32> = Vec::with_capacity(transactions.len());
        let mut tx_witness_hashes: Vec<protos::Bytes32> = Vec::with_capacity(transactions.len());
        for tx in transactions {
            tx_hashes.push(tx.hash().into());
            tx_witness_hashes.push(tx.witness_hash().into());
        }

        let tx_hashes = fbb.create_vector(&tx_hashes);
        let tx_witness_hashes = fbb.create_vector(&tx_witness_hashes);

        let mut builder = protos::StoredBlockBodyCacheBuilder::new(fbb);
        builder.add_tx_hashes(tx_hashes);
        builder.add_tx_witness_hashes(tx_witness_hashes);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredBlockBody<'a> {
    type Input = [Transaction];
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        transactions: &Self::Input,
    ) -> WIPOffset<protos::StoredBlockBody<'b>> {
        let data = protos::BlockBody::build(fbb, transactions);
        let cache = protos::StoredBlockBodyCache::build(fbb, transactions);
        let mut builder = protos::StoredBlockBodyBuilder::new(fbb);
        builder.add_data(data);
        builder.add_cache(cache);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredHeaderCache<'a> {
    type Input = Header;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        header: &Self::Input,
    ) -> WIPOffset<protos::StoredHeaderCache<'b>> {
        let hash = header.hash().into();
        let mut builder = protos::StoredHeaderCacheBuilder::new(fbb);
        builder.add_hash(&hash);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredTransactionInfo<'a> {
    type Input = TransactionInfo;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        info: &Self::Input,
    ) -> WIPOffset<protos::StoredTransactionInfo<'b>> {
        let data = info.into();
        let mut builder = protos::StoredTransactionInfoBuilder::new(fbb);
        builder.add_data(&data);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredHeader<'a> {
    type Input = Header;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        header: &Self::Input,
    ) -> WIPOffset<protos::StoredHeader<'b>> {
        let data = protos::Header::build(fbb, header);
        let cache = protos::StoredHeaderCache::build(fbb, header);
        let mut builder = protos::StoredHeaderBuilder::new(fbb);
        builder.add_data(data);
        builder.add_cache(cache);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredUncleBlocksCache<'a> {
    type Input = [UncleBlock];
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        uncles: &Self::Input,
    ) -> WIPOffset<protos::StoredUncleBlocksCache<'b>> {
        let mut hashes_vec: Vec<protos::Bytes32> = Vec::with_capacity(uncles.len());
        for uncle in uncles {
            hashes_vec.push(uncle.hash().into());
        }
        let hashes = fbb.create_vector(&hashes_vec);
        let mut builder = protos::StoredUncleBlocksCacheBuilder::new(fbb);
        builder.add_hashes(hashes);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredUncleBlocks<'a> {
    type Input = [UncleBlock];
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        uncles: &Self::Input,
    ) -> WIPOffset<protos::StoredUncleBlocks<'b>> {
        let vec = uncles
            .iter()
            .map(|uncle| protos::UncleBlock::build(fbb, uncle))
            .collect::<Vec<_>>();
        let data = fbb.create_vector(&vec);
        let cache = protos::StoredUncleBlocksCache::build(fbb, uncles);
        let mut builder = protos::StoredUncleBlocksBuilder::new(fbb);
        builder.add_data(data);
        builder.add_cache(cache);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredProposalShortIds<'a> {
    type Input = [ProposalShortId];
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        proposals: &Self::Input,
    ) -> WIPOffset<protos::StoredProposalShortIds<'b>> {
        let vec = proposals
            .iter()
            .map(Into::into)
            .collect::<Vec<protos::ProposalShortId>>();
        let data = fbb.create_vector(&vec);
        let mut builder = protos::StoredProposalShortIdsBuilder::new(fbb);
        builder.add_data(data);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredEpochExt<'a> {
    type Input = EpochExt;
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        ext: &Self::Input,
    ) -> WIPOffset<protos::StoredEpochExt<'b>> {
        let data = ext.into();
        let mut builder = protos::StoredEpochExtBuilder::new(fbb);
        builder.add_data(&data);
        builder.finish()
    }
}

impl<'a> CanBuild<'a> for protos::StoredCellMeta<'a> {
    type Input = (Capacity, H256);
    fn build<'b: 'a>(
        fbb: &mut FlatBufferBuilder<'b>,
        meta: &Self::Input,
    ) -> WIPOffset<protos::StoredCellMeta<'b>> {
        let data = meta.into();
        let mut builder = protos::StoredCellMetaBuilder::new(fbb);
        builder.add_data(&data);
        builder.finish()
    }
}
