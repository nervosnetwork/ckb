//! Advanced builders for Transaction(View), Header(View) and Block(View).

use ckb_merkle_tree::merkle_root;

use crate::{constants, core, packed, prelude::*, H256, U256};

/*
 * Definitions
 */

#[derive(Debug)]
pub struct TransactionBuilder {
    pub(crate) version: packed::Uint32,
    pub(crate) cell_deps: Vec<packed::CellDep>,
    pub(crate) header_deps: Vec<packed::Byte32>,
    pub(crate) inputs: Vec<packed::CellInput>,
    pub(crate) outputs: Vec<packed::CellOutput>,
    pub(crate) witnesses: Vec<packed::Witness>,
    pub(crate) outputs_data: Vec<packed::Bytes>,
}

#[derive(Debug)]
pub struct HeaderBuilder {
    // RawHeader
    pub(crate) version: packed::Uint32,
    pub(crate) parent_hash: packed::Byte32,
    pub(crate) timestamp: packed::Uint64,
    pub(crate) number: packed::Uint64,
    pub(crate) transactions_root: packed::Byte32,
    pub(crate) witnesses_root: packed::Byte32,
    pub(crate) proposals_hash: packed::Byte32,
    pub(crate) difficulty: packed::Byte32,
    pub(crate) uncles_hash: packed::Byte32,
    pub(crate) uncles_count: packed::Uint32,
    pub(crate) epoch: packed::Uint64,
    pub(crate) dao: packed::Byte32,
    // Nonce
    pub(crate) nonce: packed::Uint64,
}

#[derive(Debug, Default)]
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
            version: constants::HEADER_VERSION.pack(),
            parent_hash: Default::default(),
            timestamp: Default::default(),
            number: Default::default(),
            transactions_root: Default::default(),
            witnesses_root: Default::default(),
            proposals_hash: Default::default(),
            difficulty: U256::one().pack(),
            uncles_hash: Default::default(),
            uncles_count: Default::default(),
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
    ($prefix:ident, $field:ident, $type:ident) => {
        pub fn $field(mut self, v: packed::$type) -> Self {
            self.$prefix.$field = v;
            self
        }
    };
    ($field:ident, $type:ident) => {
        pub fn $field(mut self, v: packed::$type) -> Self {
            self.$field = v;
            self
        }
    };
}

macro_rules! def_setter_for_vector {
    ($field:ident, $type:ident, $func_push:ident, $func_extend:ident, $func_set:ident) => {
        pub fn $func_push(mut self, v: packed::$type) -> Self {
            self.$field.push(v);
            self
        }
        pub fn $func_extend<T>(mut self, v: T) -> Self
        where
            T: ::std::iter::IntoIterator<Item = packed::$type>
        {
            self.$field.extend(v);
            self
        }
        pub fn $func_set(mut self, v: Vec<packed::$type>) -> Self {
            self.$field= v;
            self
        }
    }
}

macro_rules! def_setter_for_view_vector {
    ($field:ident, $type:ident, $func_push:ident, $func_extend:ident, $func_set:ident) => {
        pub fn $func_push(mut self, v: core::$type) -> Self {
            self.$field.push(v);
            self
        }
        pub fn $func_extend<T>(mut self, v: T) -> Self
        where
            T: ::std::iter::IntoIterator<Item = core::$type>
        {
            self.$field.extend(v);
            self
        }
        pub fn $func_set(mut self, v: Vec<core::$type>) -> Self {
            self.$field= v;
            self
        }
    }
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
    def_setter_for_vector!(witnesses, Witness, witness, witnesses, set_witnesses);
    def_setter_for_vector!(
        outputs_data,
        Bytes,
        output_data,
        outputs_data,
        set_outputs_data
    );

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
        let hash = tx.calc_tx_hash().pack();
        let witness_hash = tx.calc_witness_hash().pack();
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
    def_setter_simple!(witnesses_root, Byte32);
    def_setter_simple!(proposals_hash, Byte32);
    def_setter_simple!(difficulty, Byte32);
    def_setter_simple!(uncles_hash, Byte32);
    def_setter_simple!(uncles_count, Uint32);
    def_setter_simple!(epoch, Uint64);
    def_setter_simple!(dao, Byte32);
    def_setter_simple!(nonce, Uint64);

    pub fn build(self) -> core::HeaderView {
        let Self {
            version,
            parent_hash,
            timestamp,
            number,
            transactions_root,
            witnesses_root,
            proposals_hash,
            difficulty,
            uncles_hash,
            uncles_count,
            epoch,
            dao,
            nonce,
        } = self;
        debug_assert!(
            Unpack::<U256>::unpack(&difficulty) > U256::zero(),
            "[HeaderBuilder] difficulty should greater than zero"
        );
        let raw = packed::RawHeader::new_builder()
            .version(version)
            .parent_hash(parent_hash)
            .timestamp(timestamp)
            .number(number)
            .transactions_root(transactions_root)
            .witnesses_root(witnesses_root)
            .proposals_hash(proposals_hash)
            .difficulty(difficulty)
            .uncles_hash(uncles_hash)
            .uncles_count(uncles_count)
            .epoch(epoch)
            .dao(dao)
            .build();
        let header = packed::Header::new_builder().raw(raw).nonce(nonce).build();
        let hash = header.calc_header_hash().pack();
        core::HeaderView { data: header, hash }
    }
}

impl BlockBuilder {
    def_setter_simple!(header, version, Uint32);
    def_setter_simple!(header, parent_hash, Byte32);
    def_setter_simple!(header, timestamp, Uint64);
    def_setter_simple!(header, number, Uint64);
    def_setter_simple!(header, transactions_root, Byte32);
    def_setter_simple!(header, witnesses_root, Byte32);
    def_setter_simple!(header, proposals_hash, Byte32);
    def_setter_simple!(header, difficulty, Byte32);
    def_setter_simple!(header, uncles_hash, Byte32);
    def_setter_simple!(header, uncles_count, Uint32);
    def_setter_simple!(header, epoch, Uint64);
    def_setter_simple!(header, dao, Byte32);
    def_setter_simple!(header, nonce, Uint64);
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
            let tx_hashes = tx_hashes.iter().map(|h| h.unpack()).collect::<Vec<H256>>();
            let tx_witness_hashes = tx_witness_hashes
                .iter()
                .map(|h| h.unpack())
                .collect::<Vec<H256>>();
            let transactions_root = merkle_root(&tx_hashes[..]);
            let witnesses_root = merkle_root(&tx_witness_hashes[..]);
            let proposals_hash = proposals.calc_proposals_hash();
            let uncles_hash = uncles.calc_uncles_hash();
            let uncles_count = uncles.len() as u32;
            header
                .transactions_root(transactions_root.pack())
                .witnesses_root(witnesses_root.pack())
                .proposals_hash(proposals_hash.pack())
                .uncles_hash(uncles_hash.pack())
                .uncles_count(uncles_count.pack())
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

    pub fn build(self) -> core::BlockView {
        self.build_internal(true)
    }

    pub fn build_unchecked(self) -> core::BlockView {
        self.build_internal(false)
    }
}

/*
 * Convert a struct to an advanced builder
 */

impl packed::Transaction {
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
    pub fn as_advanced_builder(&self) -> HeaderBuilder {
        HeaderBuilder::default()
            .version(self.raw().version())
            .parent_hash(self.raw().parent_hash())
            .timestamp(self.raw().timestamp())
            .number(self.raw().number())
            .transactions_root(self.raw().transactions_root())
            .witnesses_root(self.raw().witnesses_root())
            .proposals_hash(self.raw().proposals_hash())
            .difficulty(self.raw().difficulty())
            .uncles_hash(self.raw().uncles_hash())
            .uncles_count(self.raw().uncles_count())
            .epoch(self.raw().epoch())
            .dao(self.raw().dao())
            .nonce(self.nonce())
    }
}

impl packed::Block {
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
    pub fn as_advanced_builder(&self) -> TransactionBuilder {
        self.data().as_advanced_builder()
    }
}

impl core::HeaderView {
    pub fn as_advanced_builder(&self) -> HeaderBuilder {
        self.data().as_advanced_builder()
    }
}

impl core::BlockView {
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
