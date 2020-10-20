//! Advanced builders for Transaction(View), Header(View) and Block(View).

use crate::{
    constants, core, packed,
    prelude::*,
    utilities::{merkle_root, DIFF_TWO},
};

/*
 * Definitions
 */

/// An advanced builder for [`TransactionView`].
///
/// Base on [`packed::TransactionBuilder`] but added lots of syntactic sugar.
///
/// [`TransactionView`]: struct.TransactionView.html
/// [`packed::TransactionBuilder`]: ../packed/struct.TransactionBuilder.html
#[derive(Clone, Debug)]
pub struct TransactionBuilder {
    pub(crate) version: packed::Uint32,
    pub(crate) cell_deps: Vec<packed::CellDep>,
    pub(crate) header_deps: Vec<packed::Byte32>,
    pub(crate) inputs: Vec<packed::CellInput>,
    pub(crate) outputs: Vec<packed::CellOutput>,
    pub(crate) witnesses: Vec<packed::Bytes>,
    pub(crate) outputs_data: Vec<packed::Bytes>,
}

/// An advanced builder for [`HeaderView`].
///
/// Base on [`packed::HeaderBuilder`] but added lots of syntactic sugar.
///
/// [`HeaderView`]: struct.HeaderView.html
/// [`packed::HeaderBuilder`]: ../packed/struct.HeaderBuilder.html
#[derive(Clone, Debug)]
pub struct HeaderBuilder {
    // RawHeader
    pub(crate) version: packed::Uint32,
    pub(crate) parent_hash: packed::Byte32,
    pub(crate) timestamp: packed::Uint64,
    pub(crate) number: packed::Uint64,
    pub(crate) transactions_root: packed::Byte32,
    pub(crate) proposals_hash: packed::Byte32,
    pub(crate) compact_target: packed::Uint32,
    pub(crate) uncles_hash: packed::Byte32,
    pub(crate) epoch: packed::Uint64,
    pub(crate) dao: packed::Byte32,
    // Nonce
    pub(crate) nonce: packed::Uint128,
}

/// An advanced builder for [`BlockView`].
///
/// Base on [`packed::BlockBuilder`] but added lots of syntactic sugar.
///
/// [`BlockView`]: struct.BlockView.html
/// [`packed::BlockBuilder`]: ../packed/struct.BlockBuilder.html
#[derive(Clone, Debug, Default)]
pub struct BlockBuilder {
    pub(crate) header: HeaderBuilder,
    // Others
    pub(crate) uncles: Vec<core::UncleBlockView>,
    pub(crate) transactions: Vec<core::TransactionView>,
    pub(crate) proposals: Vec<packed::ProposalShortId>,
}

/*
 * Implement std traits.
 */

impl ::std::default::Default for TransactionBuilder {
    fn default() -> Self {
        Self {
            version: constants::TX_VERSION.pack(),
            cell_deps: Default::default(),
            header_deps: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            witnesses: Default::default(),
            outputs_data: Default::default(),
        }
    }
}

impl ::std::default::Default for HeaderBuilder {
    fn default() -> Self {
        Self {
            version: constants::BLOCK_VERSION.pack(),
            parent_hash: Default::default(),
            timestamp: Default::default(),
            number: Default::default(),
            transactions_root: Default::default(),
            proposals_hash: Default::default(),
            compact_target: DIFF_TWO.pack(),
            uncles_hash: Default::default(),
            epoch: Default::default(),
            dao: Default::default(),
            nonce: Default::default(),
        }
    }
}

/*
 * Implementations.
 */

macro_rules! def_setter_simple {
    (__add_doc, $prefix:ident, $field:ident, $type:ident, $comment:expr) => {
        #[doc = $comment]
        pub fn $field(mut self, v: packed::$type) -> Self {
            self.$prefix.$field = v;
            self
        }
    };
    (__add_doc, $field:ident, $type:ident, $comment:expr) => {
        #[doc = $comment]
        pub fn $field(mut self, v: packed::$type) -> Self {
            self.$field = v;
            self
        }
    };
    ($prefix:ident, $field:ident, $type:ident) => {
        def_setter_simple!(
            __add_doc,
            $prefix,
            $field,
            $type,
            concat!("Sets `", stringify!($prefix), ".", stringify!($field), "`.")
        );
    };
    ($field:ident, $type:ident) => {
        def_setter_simple!(
            __add_doc,
            $field,
            $type,
            concat!("Sets `", stringify!($field), "`.")
        );
    };
}

macro_rules! def_setter_for_vector {
    (
        $prefix:ident, $field:ident, $type:ident,
        $func_push:ident, $func_extend:ident, $func_set:ident,
        $comment_push:expr, $comment_extend:expr, $comment_set:expr,
    ) => {
        #[doc = $comment_push]
        pub fn $func_push(mut self, v: $prefix::$type) -> Self {
            self.$field.push(v);
            self
        }
        #[doc = $comment_extend]
        pub fn $func_extend<T>(mut self, v: T) -> Self
        where
            T: ::std::iter::IntoIterator<Item = $prefix::$type>,
        {
            self.$field.extend(v);
            self
        }
        #[doc = $comment_set]
        pub fn $func_set(mut self, v: Vec<$prefix::$type>) -> Self {
            self.$field = v;
            self
        }
    };
    ($prefix:ident, $field:ident, $type:ident, $func_push:ident, $func_extend:ident, $func_set:ident) => {
        def_setter_for_vector!(
            $prefix,
            $field,
            $type,
            $func_push,
            $func_extend,
            $func_set,
            concat!("Pushes an item into `", stringify!($field), "`."),
            concat!(
                "Extends `",
                stringify!($field),
                "` with the contents of an iterator."
            ),
            concat!("Sets `", stringify!($field), "`."),
        );
    };
    ($field:ident, $type:ident, $func_push:ident, $func_extend:ident, $func_set:ident) => {
        def_setter_for_vector!(packed, $field, $type, $func_push, $func_extend, $func_set);
    };
}

macro_rules! def_setter_for_view_vector {
    ($field:ident, $type:ident, $func_push:ident, $func_extend:ident, $func_set:ident) => {
        def_setter_for_vector!(core, $field, $type, $func_push, $func_extend, $func_set);
    };
}

impl TransactionBuilder {
    def_setter_simple!(version, Uint32);
    def_setter_for_vector!(cell_deps, CellDep, cell_dep, cell_deps, set_cell_deps);
    def_setter_for_vector!(
        header_deps,
        Byte32,
        header_dep,
        header_deps,
        set_header_deps
    );
    def_setter_for_vector!(inputs, CellInput, input, inputs, set_inputs);
    def_setter_for_vector!(outputs, CellOutput, output, outputs, set_outputs);
    def_setter_for_vector!(witnesses, Bytes, witness, witnesses, set_witnesses);
    def_setter_for_vector!(
        outputs_data,
        Bytes,
        output_data,
        outputs_data,
        set_outputs_data
    );

    /// Converts into [`TransactionView`](struct.TransactionView.html).
    pub fn build(self) -> core::TransactionView {
        let Self {
            version,
            cell_deps,
            header_deps,
            inputs,
            outputs,
            witnesses,
            outputs_data,
        } = self;
        let raw = packed::RawTransaction::new_builder()
            .version(version)
            .cell_deps(cell_deps.pack())
            .header_deps(header_deps.pack())
            .inputs(inputs.pack())
            .outputs(outputs.pack())
            .outputs_data(outputs_data.pack())
            .build();
        let tx = packed::Transaction::new_builder()
            .raw(raw)
            .witnesses(witnesses.pack())
            .build();
        let hash = tx.calc_tx_hash();
        let witness_hash = tx.calc_witness_hash();
        core::TransactionView {
            data: tx,
            hash,
            witness_hash,
        }
    }
}

impl HeaderBuilder {
    def_setter_simple!(version, Uint32);
    def_setter_simple!(parent_hash, Byte32);
    def_setter_simple!(timestamp, Uint64);
    def_setter_simple!(number, Uint64);
    def_setter_simple!(transactions_root, Byte32);
    def_setter_simple!(proposals_hash, Byte32);
    def_setter_simple!(compact_target, Uint32);
    def_setter_simple!(uncles_hash, Byte32);
    def_setter_simple!(epoch, Uint64);
    def_setter_simple!(dao, Byte32);
    def_setter_simple!(nonce, Uint128);

    /// Converts into [`HeaderView`](struct.HeaderView.html).
    pub fn build(self) -> core::HeaderView {
        let Self {
            version,
            parent_hash,
            timestamp,
            number,
            transactions_root,
            proposals_hash,
            compact_target,
            uncles_hash,
            epoch,
            dao,
            nonce,
        } = self;
        debug_assert!(
            Unpack::<u32>::unpack(&compact_target) > 0,
            "[HeaderBuilder] compact_target should greater than zero"
        );
        let raw = packed::RawHeader::new_builder()
            .version(version)
            .parent_hash(parent_hash)
            .timestamp(timestamp)
            .number(number)
            .transactions_root(transactions_root)
            .proposals_hash(proposals_hash)
            .compact_target(compact_target)
            .uncles_hash(uncles_hash)
            .epoch(epoch)
            .dao(dao)
            .build();
        let header = packed::Header::new_builder().raw(raw).nonce(nonce).build();
        let hash = header.calc_header_hash();
        core::HeaderView { data: header, hash }
    }
}

impl BlockBuilder {
    def_setter_simple!(header, version, Uint32);
    def_setter_simple!(header, parent_hash, Byte32);
    def_setter_simple!(header, timestamp, Uint64);
    def_setter_simple!(header, number, Uint64);
    def_setter_simple!(header, transactions_root, Byte32);
    def_setter_simple!(header, proposals_hash, Byte32);
    def_setter_simple!(header, compact_target, Uint32);
    def_setter_simple!(header, uncles_hash, Byte32);
    def_setter_simple!(header, epoch, Uint64);
    def_setter_simple!(header, dao, Byte32);
    def_setter_simple!(header, nonce, Uint128);
    def_setter_for_view_vector!(uncles, UncleBlockView, uncle, uncles, set_uncles);
    def_setter_for_view_vector!(
        transactions,
        TransactionView,
        transaction,
        transactions,
        set_transactions
    );
    def_setter_for_vector!(
        proposals,
        ProposalShortId,
        proposal,
        proposals,
        set_proposals
    );

    /// Set `header`.
    pub fn header(mut self, header: core::HeaderView) -> Self {
        self.header = header.as_advanced_builder();
        self
    }

    fn build_internal(self, reset_header: bool) -> core::BlockView {
        let Self {
            header,
            uncles,
            transactions,
            proposals,
        } = self;
        let (uncles, uncle_hashes) = {
            let len = uncles.len();
            uncles
                .into_iter()
                .map(|uncle_view| {
                    let core::UncleBlockView { data, hash } = uncle_view;
                    (data, hash)
                })
                .fold(
                    (Vec::with_capacity(len), Vec::with_capacity(len)),
                    |(mut uncles, mut hashes), (uncle, hash)| {
                        uncles.push(uncle);
                        hashes.push(hash);
                        (uncles, hashes)
                    },
                )
        };

        let (transactions, tx_hashes, tx_witness_hashes) = {
            let len = transactions.len();
            transactions
                .into_iter()
                .map(|tx_view| {
                    let core::TransactionView {
                        data,
                        hash,
                        witness_hash,
                    } = tx_view;
                    (data, hash, witness_hash)
                })
                .fold(
                    (
                        Vec::with_capacity(len),
                        Vec::with_capacity(len),
                        Vec::with_capacity(len),
                    ),
                    |(mut txs, mut hashes, mut witness_hashes), (tx, hash, witness_hash)| {
                        txs.push(tx);
                        hashes.push(hash);
                        witness_hashes.push(witness_hash);
                        (txs, hashes, witness_hashes)
                    },
                )
        };

        let proposals = proposals.pack();
        let uncles = uncles.pack();

        let core::HeaderView { data, hash } = if reset_header {
            let raw_transactions_root = merkle_root(&tx_hashes[..]);
            let witnesses_root = merkle_root(&tx_witness_hashes[..]);
            let transactions_root = merkle_root(&[raw_transactions_root, witnesses_root]);
            let proposals_hash = proposals.calc_proposals_hash();
            let uncles_hash = uncles.calc_uncles_hash();
            header
                .transactions_root(transactions_root)
                .proposals_hash(proposals_hash)
                .uncles_hash(uncles_hash)
                .build()
        } else {
            header.build()
        };

        let block = packed::Block::new_builder()
            .header(data)
            .uncles(uncles)
            .transactions(transactions.pack())
            .proposals(proposals)
            .build();
        core::BlockView {
            data: block,
            hash,
            uncle_hashes: uncle_hashes.pack(),
            tx_hashes,
            tx_witness_hashes,
        }
    }

    /// Converts into [`BlockView`](struct.BlockView.html) and recalculates all hashes and merkle
    /// roots in the header.
    pub fn build(self) -> core::BlockView {
        self.build_internal(true)
    }

    /// Converts into [`BlockView`](struct.BlockView.html) but does not refresh all hashes and all
    /// merkle roots in the header.
    ///
    /// # Notice
    ///
    /// [`BlockView`](struct.BlockView.html) created by this method could have invalid hashes or
    /// invalid merkle roots in the header.
    pub fn build_unchecked(self) -> core::BlockView {
        self.build_internal(false)
    }
}

/*
 * Convert a struct to an advanced builder
 */

impl packed::Transaction {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> TransactionBuilder {
        TransactionBuilder::default()
            .version(self.raw().version())
            .cell_deps(self.raw().cell_deps())
            .header_deps(self.raw().header_deps())
            .inputs(self.raw().inputs())
            .outputs(self.raw().outputs())
            .outputs_data(self.raw().outputs_data())
            .witnesses(self.witnesses())
    }
}

impl packed::Header {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> HeaderBuilder {
        HeaderBuilder::default()
            .version(self.raw().version())
            .parent_hash(self.raw().parent_hash())
            .timestamp(self.raw().timestamp())
            .number(self.raw().number())
            .transactions_root(self.raw().transactions_root())
            .proposals_hash(self.raw().proposals_hash())
            .compact_target(self.raw().compact_target())
            .uncles_hash(self.raw().uncles_hash())
            .epoch(self.raw().epoch())
            .dao(self.raw().dao())
            .nonce(self.nonce())
    }
}

impl packed::Block {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> BlockBuilder {
        BlockBuilder::default()
            .header(self.header().into_view())
            .uncles(
                self.uncles()
                    .into_iter()
                    .map(|x| x.into_view())
                    .collect::<Vec<_>>(),
            )
            .transactions(
                self.transactions()
                    .into_iter()
                    .map(|x| x.into_view())
                    .collect::<Vec<_>>(),
            )
            .proposals(self.proposals().into_iter().collect::<Vec<_>>())
    }
}

impl core::TransactionView {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> TransactionBuilder {
        self.data().as_advanced_builder()
    }
}

impl core::HeaderView {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> HeaderBuilder {
        self.data().as_advanced_builder()
    }
}

impl core::BlockView {
    /// Creates an advanced builder base on current data.
    pub fn as_advanced_builder(&self) -> BlockBuilder {
        let core::BlockView {
            data,
            uncle_hashes,
            tx_hashes,
            tx_witness_hashes,
            hash,
        } = self;
        let _ = hash;
        BlockBuilder::default()
            .header(self.header())
            .uncles(
                data.uncles()
                    .into_iter()
                    .zip(uncle_hashes.to_owned().into_iter())
                    .map(|(data, hash)| core::UncleBlockView { data, hash })
                    .collect::<Vec<_>>(),
            )
            .transactions(
                data.transactions()
                    .into_iter()
                    .zip(tx_hashes.iter())
                    .zip(tx_witness_hashes.iter())
                    .map(|((data, hash), witness_hash)| core::TransactionView {
                        data,
                        hash: hash.to_owned(),
                        witness_hash: witness_hash.to_owned(),
                    })
                    .collect::<Vec<_>>(),
            )
            .proposals(data.proposals().into_iter().collect::<Vec<_>>())
    }
}
