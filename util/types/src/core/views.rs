//! Immutable blockchain types with caches (various hashes).

use std::collections::HashSet;

use ckb_hash::new_blake2b;
use ckb_occupied_capacity::Result as CapacityResult;

use crate::{
    bytes::Bytes,
    core::{BlockNumber, Capacity, EpochNumberWithFraction, Version},
    packed,
    prelude::*,
    utilities::merkle_root,
    U256,
};

/*
 * Definitions
 *
 * ### Warning
 *
 * Please DO NOT implement `Default`, use builders to construct views.
 */

/// A readonly and immutable struct which includes [`Transaction`] and its associated hashes.
///
/// # Notice
///
/// This struct is not implement the trait [`Default`], use [`TransactionBuilder`] to construct it.
///
/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
/// [`Transaction`]: ../packed/struct.Transaction.html
/// [`TransactionBuilder`]: struct.TransactionBuilder.html
#[derive(Debug, Clone)]
pub struct TransactionView {
    pub(crate) data: packed::Transaction,
    pub(crate) hash: packed::Byte32,
    pub(crate) witness_hash: packed::Byte32,
}

/// A readonly and immutable struct which includes extra hash and the decoupled
/// parts of it.
#[derive(Debug, Clone)]
pub struct ExtraHashView {
    /// The uncles hash which is used to combine to the extra hash.
    pub(crate) uncles_hash: packed::Byte32,
    /// The first item is the new field hash, which is used to combine to the extra hash.
    /// The second item is the extra hash.
    pub(crate) extension_hash_and_extra_hash: Option<(packed::Byte32, packed::Byte32)>,
}

/// A readonly and immutable struct which includes [`Header`] and its hash.
///
/// # Notice
///
/// This struct is not implement the trait [`Default`], use [`HeaderBuilder`] to construct it.
///
/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
/// [`Header`]: ../packed/struct.Header.html
/// [`HeaderBuilder`]: struct.HeaderBuilder.html
#[derive(Debug, Clone)]
pub struct HeaderView {
    pub(crate) data: packed::Header,
    pub(crate) hash: packed::Byte32,
}

/// A readonly and immutable struct which includes [`UncleBlock`] and its hash.
///
/// # Notice
///
/// This struct is not implement the trait [`Default`], use [`BlockView::as_uncle()`] to construct it.
///
/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
/// [`UncleBlock`]: ../packed/struct.UncleBlock.html
/// [`BlockView::as_uncle()`]: struct.BlockView.html#method.as_uncle
#[derive(Debug, Clone)]
pub struct UncleBlockView {
    pub(crate) data: packed::UncleBlock,
    pub(crate) hash: packed::Byte32,
}

/// A readonly and immutable struct which includes a vector of [`UncleBlock`]s and their hashes.
///
/// # Notice
///
/// This struct is not implement the trait [`Default`], use [`BlockView::uncles()`] to construct it.
///
/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
/// [`UncleBlock`]: ../packed/struct.UncleBlock.html
/// [`BlockView::uncles()`]: struct.BlockView.html#method.uncles
#[derive(Debug, Clone)]
pub struct UncleBlockVecView {
    pub(crate) data: packed::UncleBlockVec,
    pub(crate) hashes: packed::Byte32Vec,
}

/// A readonly and immutable struct which includes [`Block`] and its associated hashes.
///
/// # Notice
///
/// This struct is not implement the trait [`Default`], use [`BlockBuilder`] to construct it.
///
/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
/// [`Block`]: ../packed/struct.Block.html
/// [`BlockBuilder`]: struct.BlockBuilder.html
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

impl ::std::fmt::Display for ExtraHashView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        if let Some((ref extension_hash, ref extra_hash)) = self.extension_hash_and_extra_hash {
            write!(
                f,
                "uncles_hash: {}, extension_hash: {}, extra_hash: {}",
                self.uncles_hash, extension_hash, extra_hash
            )
        } else {
            write!(
                f,
                "uncles_hash: {}, extension_hash: None, extra_hash: uncles_hash",
                self.uncles_hash
            )
        }
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

macro_rules! define_clone_getter {
    ($field:ident, $type:ident, $comment:expr) => {
        #[doc = $comment]
        pub fn $field(&self) -> packed::$type {
            self.$field.clone()
        }
    };
}

macro_rules! define_data_getter {
    ($type:ident) => {
        define_clone_getter!(
            data,
            $type,
            concat!(
                "Gets a clone of [`packed::",
                stringify!($type),
                "`](../packed/struct.",
                stringify!($type),
                ".html)."
            )
        );
    };
}

macro_rules! define_cache_getter {
    ($field:ident, $type:ident) => {
        define_clone_getter!(
            $field,
            $type,
            concat!("Gets a clone of `", stringify!($field), "`.")
        );
    };
}

macro_rules! define_vector_getter {
    ($field:ident, $type:ident, $comment:expr) => {
        #[doc = $comment]
        pub fn $field(&self) -> &[packed::$type] {
            &self.$field[..]
        }
    };
    ($field:ident, $type:ident) => {
        define_vector_getter!(
            $field,
            $type,
            concat!("Extracts a slice of `", stringify!($field), "`.")
        );
    };
}

macro_rules! define_inner_getter {
    (header, unpacked, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            $type,
            data().as_reader().raw().$field().unpack(),
            concat!("Gets `raw.", stringify!($field), "`.")
        );
    };
    (header, packed, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            packed::$type,
            data().raw().$field(),
            concat!("Gets `raw.", stringify!($field), "`.")
        );
    };
    (uncle, unpacked, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            $type,
            data().as_reader().header().raw().$field().unpack(),
            concat!("Gets `header.raw.", stringify!($field), "`.")
        );
    };
    (uncle, packed, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            packed::$type,
            data().header().raw().$field(),
            concat!("Gets `header.raw.", stringify!($field), "`.")
        );
    };
    (block, unpacked, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            $type,
            data().as_reader().header().raw().$field().unpack(),
            concat!("Gets `header.raw.", stringify!($field), "`.")
        );
    };
    (block, packed, $field:ident, $type:ident) => {
        define_inner_getter!(
            $field,
            packed::$type,
            data().header().raw().$field(),
            concat!("Gets `header.raw.", stringify!($field), "`.")
        );
    };
    ($field:ident, $type:path, $f0:ident()$(.$fi:ident())*, $comment:expr) => {
        #[doc = $comment]
        pub fn $field(&self) -> $type {
            self.$f0()$(.$fi())*
        }
    };
}

impl TransactionView {
    define_data_getter!(Transaction);
    define_cache_getter!(hash, Byte32);
    define_cache_getter!(witness_hash, Byte32);

    /// Gets `raw.version`.
    pub fn version(&self) -> Version {
        self.data().raw().version().unpack()
    }

    /// Gets `raw.cell_deps`.
    pub fn cell_deps(&self) -> packed::CellDepVec {
        self.data().raw().cell_deps()
    }

    /// Gets `raw.header_deps`.
    pub fn header_deps(&self) -> packed::Byte32Vec {
        self.data().raw().header_deps()
    }

    /// Gets `raw.inputs`.
    pub fn inputs(&self) -> packed::CellInputVec {
        self.data().raw().inputs()
    }

    /// Gets `raw.outputs`.
    pub fn outputs(&self) -> packed::CellOutputVec {
        self.data().raw().outputs()
    }

    /// Gets `raw.outputs_data`.
    pub fn outputs_data(&self) -> packed::BytesVec {
        self.data().raw().outputs_data()
    }

    /// Gets `witnesses`.
    pub fn witnesses(&self) -> packed::BytesVec {
        self.data().witnesses()
    }

    /// Gets an output through its index.
    pub fn output(&self, idx: usize) -> Option<packed::CellOutput> {
        self.data().raw().outputs().get(idx)
    }

    /// Gets an output and its data through its index.
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

    /// Gets out points for all outputs.
    pub fn output_pts(&self) -> Vec<packed::OutPoint> {
        (0..self.data().raw().outputs().len())
            .map(|x| packed::OutPoint::new(self.hash(), x as u32))
            .collect()
    }

    /// Creates an iterator from out points of all outputs.
    pub fn output_pts_iter(&self) -> impl Iterator<Item = packed::OutPoint> {
        let tx_hash = self.hash();
        (0..self.data().raw().outputs().len())
            .map(move |x| packed::OutPoint::new(tx_hash.clone(), x as u32))
    }

    /// Creates an iterator from out points of all inputs.
    pub fn input_pts_iter(&self) -> impl Iterator<Item = packed::OutPoint> {
        self.data()
            .raw()
            .inputs()
            .into_iter()
            .map(|x| x.previous_output())
    }

    /// Creates an iterator from all outputs and their data.
    pub fn outputs_with_data_iter(&self) -> impl Iterator<Item = (packed::CellOutput, Bytes)> {
        self.outputs()
            .into_iter()
            .zip(self.outputs_data().into_iter().map(|d| d.raw_data()))
    }

    /// Creates an iterator from `raw.cell_deps`.
    pub fn cell_deps_iter(&self) -> impl Iterator<Item = packed::CellDep> {
        self.data().raw().cell_deps().into_iter()
    }

    /// Creates an iterator from `raw.header_deps`.
    pub fn header_deps_iter(&self) -> impl Iterator<Item = packed::Byte32> {
        self.data().raw().header_deps().into_iter()
    }

    /// Sets a fake transaction hash.
    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    /// Sets a fake witness hash.
    pub fn fake_witness_hash(mut self, witness_hash: packed::Byte32) -> Self {
        self.witness_hash = witness_hash;
        self
    }

    /// Sums the capacities of all outputs.
    pub fn outputs_capacity(&self) -> CapacityResult<Capacity> {
        self.data().raw().outputs().total_capacity()
    }

    /// Checks whether the transaction is a cellbase.
    pub fn is_cellbase(&self) -> bool {
        self.data().is_cellbase()
    }

    /// Creates a new `ProposalShortId` from the transaction hash.
    pub fn proposal_short_id(&self) -> packed::ProposalShortId {
        packed::ProposalShortId::from_tx_hash(&self.hash())
    }
}

impl ExtraHashView {
    /// Creates `ExtraHashView` with `uncles_hash` and optional `extension_hash`.
    pub fn new(uncles_hash: packed::Byte32, extension_hash_opt: Option<packed::Byte32>) -> Self {
        let extension_hash_and_extra_hash = extension_hash_opt.map(|extension_hash| {
            let mut ret = [0u8; 32];
            let mut blake2b = new_blake2b();
            blake2b.update(uncles_hash.as_slice());
            blake2b.update(extension_hash.as_slice());
            blake2b.finalize(&mut ret);
            (extension_hash, ret.pack())
        });
        Self {
            uncles_hash,
            extension_hash_and_extra_hash,
        }
    }

    /// Gets `uncles_hash`.
    pub fn uncles_hash(&self) -> packed::Byte32 {
        self.uncles_hash.clone()
    }

    /// Gets `extension_hash`.
    pub fn extension_hash(&self) -> Option<packed::Byte32> {
        self.extension_hash_and_extra_hash
            .as_ref()
            .map(|(ref extension_hash, _)| extension_hash.clone())
    }

    /// Gets `extra_hash`.
    pub fn extra_hash(&self) -> packed::Byte32 {
        self.extension_hash_and_extra_hash
            .as_ref()
            .map(|(_, ref extra_hash)| extra_hash.clone())
            .unwrap_or_else(|| self.uncles_hash.clone())
    }
}

impl HeaderView {
    define_data_getter!(Header);
    define_cache_getter!(hash, Byte32);

    define_inner_getter!(header, unpacked, version, Version);
    define_inner_getter!(header, unpacked, number, BlockNumber);
    define_inner_getter!(header, unpacked, compact_target, u32);
    define_inner_getter!(header, unpacked, timestamp, u64);
    define_inner_getter!(header, unpacked, epoch, EpochNumberWithFraction);

    define_inner_getter!(header, packed, parent_hash, Byte32);
    define_inner_getter!(header, packed, transactions_root, Byte32);
    define_inner_getter!(header, packed, proposals_hash, Byte32);
    define_inner_getter!(header, packed, extra_hash, Byte32);
    define_inner_getter!(header, packed, dao, Byte32);

    /// Gets `raw.difficulty`.
    pub fn difficulty(&self) -> U256 {
        self.data().raw().difficulty()
    }

    /// Gets `nonce`.
    pub fn nonce(&self) -> u128 {
        self.data().nonce().unpack()
    }

    /// Checks whether the header is the header block.
    pub fn is_genesis(&self) -> bool {
        self.number() == 0
    }

    /// Sets a fake header hash.
    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }
}

impl UncleBlockView {
    define_data_getter!(UncleBlock);
    define_cache_getter!(hash, Byte32);

    define_inner_getter!(uncle, unpacked, version, Version);
    define_inner_getter!(uncle, unpacked, number, BlockNumber);
    define_inner_getter!(uncle, unpacked, compact_target, u32);
    define_inner_getter!(uncle, unpacked, timestamp, u64);
    define_inner_getter!(uncle, unpacked, epoch, EpochNumberWithFraction);

    define_inner_getter!(uncle, packed, parent_hash, Byte32);
    define_inner_getter!(uncle, packed, transactions_root, Byte32);
    define_inner_getter!(uncle, packed, proposals_hash, Byte32);
    define_inner_getter!(uncle, packed, extra_hash, Byte32);
    define_inner_getter!(uncle, packed, dao, Byte32);

    /// Gets `header.raw.difficulty`.
    pub fn difficulty(&self) -> U256 {
        self.data().header().raw().difficulty()
    }

    /// Gets `header.nonce`.
    pub fn nonce(&self) -> u128 {
        self.data().header().nonce().unpack()
    }

    /// Gets `header`.
    pub fn header(&self) -> HeaderView {
        HeaderView {
            data: self.data.header(),
            hash: self.hash(),
        }
    }

    /// Sets a fake hash.
    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    /// Calculates the hash for proposals.
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.data().as_reader().calc_proposals_hash()
    }

    /// Calculates the serialized size of a UncleBlock in Block.
    pub fn serialized_size_in_block() -> usize {
        packed::UncleBlock::serialized_size_in_block()
    }
}

impl UncleBlockVecView {
    define_data_getter!(UncleBlockVec);
    define_cache_getter!(hashes, Byte32Vec);

    /// Gets an uncle block through its index.
    pub fn get(&self, index: usize) -> Option<UncleBlockView> {
        if index >= self.data().len() {
            None
        } else {
            Some(self.get_unchecked(index))
        }
    }

    /// Gets an uncle block through its index without checks.
    ///
    /// # Panics
    ///
    /// Panics if the index out of range.
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

impl BlockView {
    define_data_getter!(Block);
    define_cache_getter!(hash, Byte32);
    define_cache_getter!(uncle_hashes, Byte32Vec);

    define_vector_getter!(tx_hashes, Byte32);
    define_vector_getter!(tx_witness_hashes, Byte32);

    define_inner_getter!(block, unpacked, version, Version);
    define_inner_getter!(block, unpacked, number, BlockNumber);
    define_inner_getter!(block, unpacked, compact_target, u32);
    define_inner_getter!(block, unpacked, timestamp, u64);
    define_inner_getter!(block, unpacked, epoch, EpochNumberWithFraction);

    define_inner_getter!(block, packed, parent_hash, Byte32);
    define_inner_getter!(block, packed, transactions_root, Byte32);
    define_inner_getter!(block, packed, proposals_hash, Byte32);
    define_inner_getter!(block, packed, extra_hash, Byte32);
    define_inner_getter!(block, packed, dao, Byte32);

    /// Gets `header.nonce`.
    pub fn nonce(&self) -> u128 {
        self.data().header().nonce().unpack()
    }

    /// Gets `header.difficulty`.
    pub fn difficulty(&self) -> U256 {
        self.header().difficulty()
    }

    /// Gets `header`.
    pub fn header(&self) -> HeaderView {
        HeaderView {
            data: self.data.header(),
            hash: self.hash(),
        }
    }

    /// Gets `uncles`.
    pub fn uncles(&self) -> UncleBlockVecView {
        UncleBlockVecView {
            data: self.data.uncles(),
            hashes: self.uncle_hashes(),
        }
    }

    /// Gets `extension`.
    ///
    /// # Panics
    ///
    /// Panics if the extension exists but not a valid [`Bytes`](../packed/struct.Bytes.html).
    pub fn extension(&self) -> Option<packed::Bytes> {
        self.data.extension()
    }

    /// Converts into an uncle block.
    pub fn as_uncle(&self) -> UncleBlockView {
        UncleBlockView {
            data: self.data.as_uncle(),
            hash: self.hash(),
        }
    }

    /// Gets `transactions`.
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

    /// Creates an iterator from `proposals` of the block and `proposals` of `uncles`.
    pub fn union_proposal_ids_iter(&self) -> impl Iterator<Item = packed::ProposalShortId> {
        self.data().proposals().into_iter().chain(
            self.data()
                .uncles()
                .into_iter()
                .flat_map(|u| u.proposals().into_iter()),
        )
    }

    /// Creates a hashset from `proposals` of the block and `proposals` of `uncles`.
    pub fn union_proposal_ids(&self) -> HashSet<packed::ProposalShortId> {
        self.union_proposal_ids_iter().collect()
    }

    /// Gets a transaction through its index.
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

    /// Gets an output through its transaction index and its own index.
    pub fn output(&self, tx_index: usize, index: usize) -> Option<packed::CellOutput> {
        self.data
            .transactions()
            .get(tx_index)
            .and_then(|tx| tx.raw().outputs().get(index))
    }

    /// Sets a fake header hash.
    pub fn fake_hash(mut self, hash: packed::Byte32) -> Self {
        self.hash = hash;
        self
    }

    /// Checks whether the block is the genesis block.
    pub fn is_genesis(&self) -> bool {
        self.number() == 0
    }

    /// Calculates the hash for uncle blocks.
    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        self.data().as_reader().calc_uncles_hash()
    }

    /// Calculates the hash for extension.
    pub fn calc_extension_hash(&self) -> Option<packed::Byte32> {
        self.data().as_reader().calc_extension_hash()
    }

    /// Calculates the extra hash.
    pub fn calc_extra_hash(&self) -> ExtraHashView {
        self.data().as_reader().calc_extra_hash()
    }

    /// Calculates the hash for proposals.
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.data().as_reader().calc_proposals_hash()
    }

    /// Calculates the merkel root for transactions with witnesses.
    pub fn calc_transactions_root(&self) -> packed::Byte32 {
        merkle_root(&[
            self.calc_raw_transactions_root(),
            self.calc_witnesses_root(),
        ])
    }

    /// Calculates the merkle root for transactions without witnesses.
    pub fn calc_raw_transactions_root(&self) -> packed::Byte32 {
        merkle_root(&self.tx_hashes[..])
    }

    /// Calculates the merkle root for transaction witnesses.
    pub fn calc_witnesses_root(&self) -> packed::Byte32 {
        merkle_root(&self.tx_witness_hashes[..])
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
    /// Creates a new `BlockView`.
    ///
    /// # Notice
    ///
    /// [`BlockView`] created by this method could have invalid hashes or
    /// invalid merkle roots in the header.
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

    /// Creates a new `BlockView` with a extension.
    ///
    /// # Notice
    ///
    /// [`BlockView`] created by this method could have invalid hashes or
    /// invalid merkle roots in the header.
    pub fn new_unchecked_with_extension(
        header: HeaderView,
        uncles: UncleBlockVecView,
        body: Vec<TransactionView>,
        proposals: packed::ProposalShortIdVec,
        extension: packed::Bytes,
    ) -> Self {
        let block = packed::BlockV1::new_builder()
            .header(header.data())
            .transactions(body.iter().map(|tx| tx.data()).pack())
            .uncles(uncles.data())
            .proposals(proposals)
            .extension(extension)
            .build()
            .as_v0();
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
    /// Calculates the associated hashes and converts into [`TransactionView`] with those hashes.
    ///
    /// [`TransactionView`]: ../core/struct.TransactionView.html
    pub fn into_view(self) -> TransactionView {
        let hash = self.calc_tx_hash();
        let witness_hash = self.calc_witness_hash();
        TransactionView {
            data: self,
            hash,
            witness_hash,
        }
    }
}

impl packed::Header {
    /// Calculates the header hash and converts into [`HeaderView`] with the hash.
    ///
    /// [`HeaderView`]: ../core/struct.HeaderView.html
    pub fn into_view(self) -> HeaderView {
        let hash = self.calc_header_hash();
        HeaderView { data: self, hash }
    }
}

impl packed::UncleBlock {
    /// Calculates the header hash and converts into [`UncleBlockView`] with the hash.
    ///
    /// [`UncleBlockView`]: ../core/struct.UncleBlockView.html
    pub fn into_view(self) -> UncleBlockView {
        let hash = self.calc_header_hash();
        UncleBlockView { data: self, hash }
    }
}

impl packed::Block {
    /// Calculates transaction associated hashes and converts them into [`BlockView`].
    ///
    /// # Notice
    ///
    /// [`BlockView`] created by this method could have invalid hashes or
    /// invalid merkle roots in the header.
    ///
    /// [`BlockView`]: ../core/struct.BlockView.html
    pub fn into_view_without_reset_header(self) -> BlockView {
        let tx_hashes = self.calc_tx_hashes();
        let tx_witness_hashes = self.calc_tx_witness_hashes();
        Self::block_into_view_internal(self, tx_hashes, tx_witness_hashes)
    }

    /// Calculates transaction associated hashes, resets all hashes and merkle roots in the header, then converts them into [`BlockView`].
    ///
    /// [`BlockView`]: ../core/struct.BlockView.html
    pub fn into_view(self) -> BlockView {
        let tx_hashes = self.calc_tx_hashes();
        let tx_witness_hashes = self.calc_tx_witness_hashes();
        let block = self.reset_header_with_hashes(&tx_hashes[..], &tx_witness_hashes[..]);
        Self::block_into_view_internal(block, tx_hashes, tx_witness_hashes)
    }

    fn block_into_view_internal(
        block: packed::Block,
        tx_hashes: Vec<packed::Byte32>,
        tx_witness_hashes: Vec<packed::Byte32>,
    ) -> BlockView {
        let hash = block.as_reader().calc_header_hash();
        let uncle_hashes = block
            .as_reader()
            .uncles()
            .iter()
            .map(|uncle| uncle.calc_header_hash())
            .pack();
        BlockView {
            data: block,
            hash,
            uncle_hashes,
            tx_hashes,
            tx_witness_hashes,
        }
    }
}
