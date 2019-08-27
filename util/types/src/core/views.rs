//! Immutable blockchain types with caches (various hashes).

use std::collections::HashSet;

use ckb_merkle_tree::merkle_root;
use ckb_occupied_capacity::Result as CapacityResult;

use crate::{
    bytes::Bytes,
    core::{BlockNumber, Capacity, EpochNumber, Version},
    packed,
    prelude::*,
    H256, U256,
};

/*
 * Definitions
 *
 * ### Warning
 *
 * Please DO NOT implement `Default`, use builders to construct views.
 */

#[derive(Debug, Clone)]
pub struct TransactionView {
    pub(crate) data: packed::Transaction,
    pub(crate) hash: packed::Byte32,
    pub(crate) witness_hash: packed::Byte32,
}

#[derive(Debug, Clone)]
pub struct HeaderView {
    pub(crate) data: packed::Header,
    pub(crate) hash: packed::Byte32,
}

#[derive(Debug, Clone)]
pub struct UncleBlockView {
    pub(crate) data: packed::UncleBlock,
    pub(crate) hash: packed::Byte32,
}

#[derive(Debug, Clone)]
pub struct UncleBlockVecView {
    pub(crate) data: packed::UncleBlockVec,
    pub(crate) hashes: packed::Byte32Vec,
}

#[derive(Debug, Clone)]
pub struct BlockView {
    pub(crate) data: packed::Block,
    pub(crate) hash: packed::Byte32,
    pub(crate) uncle_hashes: packed::Byte32Vec,
    pub(crate) tx_hashes: Vec<packed::Byte32>,
    pub(crate) tx_witness_hashes: Vec<packed::Byte32>,
}

/*
 * Implement std traits.
 */

impl ::std::fmt::Display for TransactionView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(
            f,
            "TransactionView {{ data: {}, hash: {}, witness_hash: {} }}",
            self.data, self.hash, self.witness_hash
        )
    }
}

impl ::std::fmt::Display for HeaderView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(
            f,
            "HeaderView {{ data: {}, hash: {} }}",
            self.data, self.hash
        )
    }
}

impl ::std::fmt::Display for UncleBlockView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(
            f,
            "UncleBlockView {{ data: {}, hash: {} }}",
            self.data, self.hash
        )
    }
}

impl ::std::fmt::Display for UncleBlockVecView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(
            f,
            "UncleBlockVecView {{ data: {}, hashes: {} }}",
            self.data, self.hashes
        )
    }
}

impl ::std::fmt::Display for BlockView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(
            f,
            "BlockView {{ data: {}, hash: {}, uncle_hashes: {},\
             tx_hashes: {:?}, tx_witness_hashes: {:?} }}",
            self.data, self.hash, self.uncle_hashes, self.tx_hashes, self.tx_witness_hashes
        )
    }
}

/*
 * Define getters
 */

macro_rules! define_simple_getter {
    ($field:ident, $type:ident) => {
        pub fn $field(&self) -> packed::$type {
            self.$field.clone()
        }
    }
}

macro_rules! define_vector_getter {
    ($field:ident, $type:ident) => {
        pub fn $field(&self) -> &[packed::$type] {
            &self.$field[..]
        }
    }
}

impl TransactionView {
    define_simple_getter!(data, Transaction);
    define_simple_getter!(hash, Byte32);
    define_simple_getter!(witness_hash, Byte32);

    pub fn version(&self) -> Version {
        self.data().raw().version().unpack()
    }

    pub fn cell_deps(&self) -> packed::CellDepVec {
        self.data().raw().cell_deps()
    }

    pub fn header_deps(&self) -> packed::Byte32Vec {
        self.data().raw().header_deps()
    }

    pub fn inputs(&self) -> packed::CellInputVec {
        self.data().raw().inputs()
    }

    pub fn outputs(&self) -> packed::CellOutputVec {
        self.data().raw().outputs()
    }

    pub fn outputs_data(&self) -> packed::BytesVec {
        self.data().raw().outputs_data()
    }

    pub fn witnesses(&self) -> packed::WitnessVec {
        self.data().witnesses()
    }

    pub fn output(&self, idx: usize) -> Option<packed::CellOutput> {
        self.data().raw().outputs().get(idx)
    }

    pub fn output_with_data(&self, idx: usize) -> Option<(packed::CellOutput, Bytes)> {
        self.data().raw().outputs().get(idx).map(|output| {
            let data = self
                .data()
                .raw()
                .outputs_data()
                .get(idx)
                .should_be_ok()
                .raw_data();
            (output, data)
        })
    }

    pub fn output_pts(&self) -> Vec<packed::OutPoint> {
        let h: H256 = self.hash().unpack();
        (0..self.data().raw().outputs().len())
            .map(|x| packed::OutPoint::new(h.clone(), x as u32))
            .collect()
    }

    pub fn input_pts_iter(&self) -> impl Iterator<Item = packed::OutPoint> {
        self.data()
            .raw()
            .inputs()
            .into_iter()
            .map(|x| x.previous_output())
    }

    pub fn outputs_with_data_iter(&self) -> impl Iterator<Item = (packed::CellOutput, Bytes)> {
        self.outputs()
            .into_iter()
            .zip(self.outputs_data().into_iter().map(|d| d.raw_data()))
    }

    pub fn cell_deps_iter(&self) -> impl Iterator<Item = packed::CellDep> {
        self.data().raw().cell_deps().into_iter()
    }

    pub fn header_deps_iter(&self) -> impl Iterator<Item = packed::Byte32> {
        self.data().raw().header_deps().into_iter()
    }

    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    pub fn fake_witness_hash(mut self, witness_hash: packed::Byte32) -> Self {
        self.witness_hash = witness_hash;
        self
    }

    pub fn outputs_capacity(&self) -> CapacityResult<Capacity> {
        self.data().raw().outputs().total_capacity()
    }

    pub fn is_cellbase(&self) -> bool {
        self.data().is_cellbase()
    }

    pub fn is_empty(&self) -> bool {
        let raw = self.data().raw();
        raw.inputs().is_empty() || raw.outputs().is_empty()
    }

    pub fn proposal_short_id(&self) -> packed::ProposalShortId {
        packed::ProposalShortId::from_tx_hash(&self.hash().unpack())
    }

    pub fn serialized_size(&self) -> usize {
        self.data().serialized_size()
    }
}

macro_rules! define_header_unpacked_inner_getter {
    ($field:ident, $type:ident) => {
        pub fn $field(&self) -> $type {
            self.data().as_reader().raw().$field().unpack()
        }
    }
}

impl HeaderView {
    define_simple_getter!(data, Header);
    define_simple_getter!(hash, Byte32);

    define_header_unpacked_inner_getter!(version, Version);
    define_header_unpacked_inner_getter!(number, BlockNumber);
    define_header_unpacked_inner_getter!(difficulty, U256);
    define_header_unpacked_inner_getter!(timestamp, u64);
    define_header_unpacked_inner_getter!(parent_hash, H256);
    define_header_unpacked_inner_getter!(transactions_root, H256);
    define_header_unpacked_inner_getter!(witnesses_root, H256);
    define_header_unpacked_inner_getter!(proposals_hash, H256);
    define_header_unpacked_inner_getter!(uncles_hash, H256);
    define_header_unpacked_inner_getter!(uncles_count, u32);
    define_header_unpacked_inner_getter!(epoch, EpochNumber);

    pub fn dao(&self) -> packed::Byte32 {
        self.data().raw().dao()
    }

    pub fn nonce(&self) -> u64 {
        self.data().nonce().unpack()
    }

    pub fn is_genesis(&self) -> bool {
        self.number() == 0
    }

    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }
}

macro_rules! define_uncle_unpacked_inner_getter {
    ($field:ident, $type:ident) => {
        pub fn $field(&self) -> $type {
            self.data().as_reader().header().raw().$field().unpack()
        }
    }
}

impl UncleBlockView {
    define_simple_getter!(data, UncleBlock);
    define_simple_getter!(hash, Byte32);

    define_uncle_unpacked_inner_getter!(version, Version);
    define_uncle_unpacked_inner_getter!(number, BlockNumber);
    define_uncle_unpacked_inner_getter!(difficulty, U256);
    define_uncle_unpacked_inner_getter!(timestamp, u64);
    define_uncle_unpacked_inner_getter!(parent_hash, H256);
    define_uncle_unpacked_inner_getter!(transactions_root, H256);
    define_uncle_unpacked_inner_getter!(witnesses_root, H256);
    define_uncle_unpacked_inner_getter!(proposals_hash, H256);
    define_uncle_unpacked_inner_getter!(uncles_hash, H256);
    define_uncle_unpacked_inner_getter!(uncles_count, u32);
    define_uncle_unpacked_inner_getter!(epoch, EpochNumber);

    pub fn dao(&self) -> packed::Byte32 {
        self.data().header().raw().dao()
    }

    pub fn nonce(&self) -> u64 {
        self.data().header().nonce().unpack()
    }

    pub fn header(&self) -> HeaderView {
        HeaderView {
            data: self.data.header(),
            hash: self.hash(),
        }
    }

    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.data().as_reader().calc_proposals_hash()
    }
}

impl UncleBlockVecView {
    define_simple_getter!(data, UncleBlockVec);
    define_simple_getter!(hashes, Byte32Vec);

    pub fn get(&self, index: usize) -> Option<UncleBlockView> {
        if index >= self.data().len() {
            None
        } else {
            Some(self.get_unchecked(index))
        }
    }

    pub fn get_unchecked(&self, index: usize) -> UncleBlockView {
        let data = self.data().get(index).should_be_ok();
        let hash = self.hashes().get(index).should_be_ok();
        UncleBlockView { data, hash }
    }
}

pub struct UncleBlockVecViewIterator(UncleBlockVecView, usize, usize);

impl ::std::iter::Iterator for UncleBlockVecViewIterator {
    type Item = UncleBlockView;
    fn next(&mut self) -> Option<Self::Item> {
        if self.1 >= self.2 {
            None
        } else {
            let index = self.1;
            self.1 += 1;
            let data = self.0.data().get(index).should_be_ok();
            let hash = self.0.hashes().get(index).should_be_ok();
            Some(UncleBlockView { data, hash })
        }
    }
}

impl ::std::iter::ExactSizeIterator for UncleBlockVecViewIterator {
    fn len(&self) -> usize {
        self.2 - self.1
    }
}

impl ::std::iter::IntoIterator for UncleBlockVecView {
    type Item = UncleBlockView;
    type IntoIter = UncleBlockVecViewIterator;
    fn into_iter(self) -> Self::IntoIter {
        let len = self.data().len();
        UncleBlockVecViewIterator(self, 0, len)
    }
}

macro_rules! define_block_unpacked_inner_getter {
    ($field:ident, $type:ident) => {
        pub fn $field(&self) -> $type {
            self.data().as_reader().header().raw().$field().unpack()
        }
    }
}

impl BlockView {
    define_simple_getter!(data, Block);
    define_simple_getter!(hash, Byte32);
    define_simple_getter!(uncle_hashes, Byte32Vec);

    define_vector_getter!(tx_hashes, Byte32);
    define_vector_getter!(tx_witness_hashes, Byte32);

    define_block_unpacked_inner_getter!(version, Version);
    define_block_unpacked_inner_getter!(number, BlockNumber);
    define_block_unpacked_inner_getter!(difficulty, U256);
    define_block_unpacked_inner_getter!(timestamp, u64);
    define_block_unpacked_inner_getter!(parent_hash, H256);
    define_block_unpacked_inner_getter!(transactions_root, H256);
    define_block_unpacked_inner_getter!(witnesses_root, H256);
    define_block_unpacked_inner_getter!(proposals_hash, H256);
    define_block_unpacked_inner_getter!(uncles_hash, H256);
    define_block_unpacked_inner_getter!(uncles_count, u32);
    define_block_unpacked_inner_getter!(epoch, EpochNumber);

    pub fn dao(&self) -> packed::Byte32 {
        self.data().header().raw().dao()
    }

    pub fn nonce(&self) -> u64 {
        self.data().header().nonce().unpack()
    }

    pub fn header(&self) -> HeaderView {
        HeaderView {
            data: self.data.header(),
            hash: self.hash(),
        }
    }

    pub fn uncles(&self) -> UncleBlockVecView {
        UncleBlockVecView {
            data: self.data.uncles(),
            hashes: self.uncle_hashes(),
        }
    }

    pub fn as_uncle(&self) -> UncleBlockView {
        UncleBlockView {
            data: self.data.as_uncle(),
            hash: self.hash(),
        }
    }

    pub fn transactions(&self) -> Vec<TransactionView> {
        self.data
            .transactions()
            .into_iter()
            .zip(self.tx_hashes().iter())
            .zip(self.tx_witness_hashes().iter())
            .map(|((data, hash), witness_hash)| TransactionView {
                data,
                hash: hash.to_owned(),
                witness_hash: witness_hash.to_owned(),
            })
            .collect()
    }

    pub fn union_proposal_ids_iter(&self) -> impl Iterator<Item = packed::ProposalShortId> {
        self.data().proposals().into_iter().chain(
            self.data()
                .uncles()
                .into_iter()
                .flat_map(|u| u.proposals().into_iter()),
        )
    }

    pub fn union_proposal_ids(&self) -> HashSet<packed::ProposalShortId> {
        self.union_proposal_ids_iter().collect()
    }

    pub fn transaction(&self, index: usize) -> Option<TransactionView> {
        self.data.transactions().get(index).map(|data| {
            let hash = self.tx_hashes.get(index).should_be_ok().to_owned();
            let witness_hash = self.tx_witness_hashes.get(index).should_be_ok().to_owned();
            TransactionView {
                data,
                hash,
                witness_hash,
            }
        })
    }

    pub fn output(&self, tx_index: usize, index: usize) -> Option<packed::CellOutput> {
        self.data
            .transactions()
            .get(tx_index)
            .and_then(|tx| tx.raw().outputs().get(index))
    }

    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    pub fn is_genesis(&self) -> bool {
        self.number() == 0
    }

    pub fn calc_uncles_hash(&self) -> H256 {
        self.data().as_reader().calc_uncles_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.data().as_reader().calc_proposals_hash()
    }

    pub fn calc_transactions_root(&self) -> H256 {
        let tx_hashes = self
            .tx_hashes()
            .iter()
            .map(|x| x.unpack())
            .collect::<Vec<H256>>();
        merkle_root(&tx_hashes[..])
    }

    pub fn calc_witnesses_root(&self) -> H256 {
        let tx_witness_hashes = self
            .tx_witness_hashes()
            .iter()
            .map(|x| x.unpack())
            .collect::<Vec<H256>>();
        merkle_root(&tx_witness_hashes[..])
    }

    pub fn serialized_size(&self) -> usize {
        self.data().serialized_size()
    }
}

/*
 * Implement std traits
 */

macro_rules! impl_std_cmp_eq_and_hash {
    ($struct:ident, $field:ident) => {
        impl PartialEq for $struct {
            fn eq(&self, other: &Self) -> bool {
                self.$field.as_slice() == other.$field.as_slice()
            }
        }
        impl Eq for $struct {}

        impl ::std::hash::Hash for $struct {
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                state.write(self.$field().as_slice())
            }
        }
    };
}

impl_std_cmp_eq_and_hash!(TransactionView, witness_hash);
impl_std_cmp_eq_and_hash!(HeaderView, hash);
impl_std_cmp_eq_and_hash!(UncleBlockView, hash);
impl_std_cmp_eq_and_hash!(BlockView, hash);

/*
 * Methods for views
 */

impl BlockView {
    pub fn new_unchecked(
        header: HeaderView,
        uncles: UncleBlockVecView,
        body: Vec<TransactionView>,
        proposals: packed::ProposalShortIdVec,
    ) -> Self {
        let block = packed::Block::new_builder()
            .header(header.data())
            .transactions(body.iter().map(|tx| tx.data()).pack())
            .uncles(uncles.data())
            .proposals(proposals)
            .build();
        let tx_hashes = body.iter().map(|tx| tx.hash()).collect::<Vec<_>>();
        let tx_witness_hashes = body.iter().map(|tx| tx.witness_hash()).collect::<Vec<_>>();
        Self {
            data: block,
            hash: header.hash(),
            uncle_hashes: uncles.hashes(),
            tx_hashes,
            tx_witness_hashes,
        }
    }
}

/*
 * Convert packed bytes wrappers to views.
 */

impl packed::Transaction {
    pub fn into_view(self) -> TransactionView {
        let hash = self.calc_tx_hash().pack();
        let witness_hash = self.calc_witness_hash().pack();
        TransactionView {
            data: self,
            hash,
            witness_hash,
        }
    }
}

impl packed::Header {
    pub fn into_view(self) -> HeaderView {
        let hash = self.calc_header_hash().pack();
        HeaderView { data: self, hash }
    }
}

impl packed::UncleBlock {
    pub fn into_view(self) -> UncleBlockView {
        let hash = self.calc_header_hash().pack();
        UncleBlockView { data: self, hash }
    }
}

impl packed::Block {
    pub fn into_view_without_reset_header(self) -> BlockView {
        let tx_hashes = self.calc_tx_hashes();
        let tx_witness_hashes = self.calc_tx_witness_hashes();
        Self::block_into_view_internal(self, &tx_hashes[..], &tx_witness_hashes[..])
    }

    pub fn into_view(self) -> BlockView {
        let tx_hashes = self.calc_tx_hashes();
        let tx_witness_hashes = self.calc_tx_witness_hashes();
        let block = self.reset_header_with_hashes(&tx_hashes[..], &tx_witness_hashes[..]);
        Self::block_into_view_internal(block, &tx_hashes[..], &tx_witness_hashes[..])
    }

    fn block_into_view_internal(
        block: packed::Block,
        tx_hashes: &[H256],
        tx_witness_hashes: &[H256],
    ) -> BlockView {
        let hash = block.as_reader().calc_header_hash().pack();
        let uncle_hashes = block
            .as_reader()
            .uncles()
            .iter()
            .map(|uncle| uncle.calc_header_hash().pack())
            .pack();
        let tx_hashes = tx_hashes.iter().map(Pack::pack).collect::<Vec<_>>();
        let tx_witness_hashes = tx_witness_hashes.iter().map(Pack::pack).collect::<Vec<_>>();
        BlockView {
            data: block,
            hash,
            uncle_hashes,
            tx_hashes,
            tx_witness_hashes,
        }
    }
}
