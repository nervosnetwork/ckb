use ckb_hash::{blake2b_256, new_blake2b};

use crate::{core, packed, prelude::*};

/*
 * Calculate simple hash for packed bytes wrappers.
 */

// Do NOT use this trait directly.
// Please call the methods which specify the hash content.
pub(crate) trait CalcHash {
    fn calc_hash(&self) -> packed::Byte32;
}

impl<'r, R> CalcHash for R
where
    R: Reader<'r>,
{
    fn calc_hash(&self) -> packed::Byte32 {
        blake2b_256(self.as_slice()).pack()
    }
}

/*
 * Calculate special hash for packed bytes wrappers.
 */

macro_rules! impl_calc_special_hash_for_entity {
    ($entity:ident, $func_name:ident, $return:ty, $comment:expr) => {
        impl packed::$entity {
            #[doc = $comment]
            pub fn $func_name(&self) -> $return {
                self.as_reader().$func_name()
            }
        }
    };
    ($entity:ident, $func_name:ident, $return:ty) => {
        impl_calc_special_hash_for_entity!(
            $entity,
            $func_name,
            $return,
            concat!(
                "Calls [`",
                stringify!($entity),
                "Reader.",
                stringify!($func_name),
                "()`](struct.",
                stringify!($entity),
                "Reader.html#method.",
                stringify!($func_name),
                ") for [`self.as_reader()`](#method.as_reader)."
            )
        );
    };
    ($entity:ident, $func_name:ident) => {
        impl_calc_special_hash_for_entity!($entity, $func_name, packed::Byte32);
    };
}

impl packed::CellOutput {
    /// Calculates the hash for cell data.
    ///
    /// Returns the empty hash if no data, otherwise, calculates the hash of the data and returns it.
    pub fn calc_data_hash(data: &[u8]) -> packed::Byte32 {
        if data.is_empty() {
            packed::Byte32::zero()
        } else {
            blake2b_256(data).pack()
        }
    }
}

impl<'r> packed::BytesReader<'r> {
    /// Calculates the hash for raw data in `Bytes`.
    ///
    /// Returns the empty hash if no data, otherwise, calculates the hash of the data and returns it.
    pub fn calc_raw_data_hash(&self) -> packed::Byte32 {
        blake2b_256(self.raw_data()).pack()
    }
}
impl_calc_special_hash_for_entity!(Bytes, calc_raw_data_hash);

impl<'r> packed::ScriptReader<'r> {
    /// Calculates the hash for [self.as_slice()] as the script hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_script_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Script, calc_script_hash);

impl<'r> packed::CellOutputReader<'r> {
    /// Calls [`ScriptReader.calc_script_hash()`] for [`self.lock()`].
    ///
    /// [`ScriptReader.calc_script_hash()`]: struct.ScriptReader.html#method.calc_script_hash
    /// [`self.lock()`]: #method.lock
    pub fn calc_lock_hash(&self) -> packed::Byte32 {
        self.lock().calc_script_hash()
    }
}
impl_calc_special_hash_for_entity!(CellOutput, calc_lock_hash);

impl<'r> packed::ProposalShortIdVecReader<'r> {
    /// Calculates the hash for proposals.
    ///
    /// Returns the empty hash if no proposals short ids, otherwise, calculates a hash for all
    /// proposals short ids and return it.
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        if self.is_empty() {
            packed::Byte32::zero()
        } else {
            let mut ret = [0u8; 32];
            let mut blake2b = new_blake2b();
            for id in self.iter() {
                blake2b.update(id.as_slice());
            }
            blake2b.finalize(&mut ret);
            ret.pack()
        }
    }
}
impl_calc_special_hash_for_entity!(ProposalShortIdVec, calc_proposals_hash);

impl<'r> packed::RawTransactionReader<'r> {
    /// Calculates the hash for [self.as_slice()] as the transaction hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_tx_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawTransaction, calc_tx_hash);

impl<'r> packed::TransactionReader<'r> {
    /// Calls [`RawTransactionReader.calc_tx_hash()`] for [`self.raw()`].
    ///
    /// [`RawTransactionReader.calc_tx_hash()`]: struct.RawTransactionReader.html#method.calc_tx_hash
    /// [`self.raw()`]: #method.raw
    pub fn calc_tx_hash(&self) -> packed::Byte32 {
        self.raw().calc_tx_hash()
    }

    /// Calculates the hash for [self.as_slice()] as the witness hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_witness_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Transaction, calc_tx_hash);
impl_calc_special_hash_for_entity!(Transaction, calc_witness_hash);

impl<'r> packed::RawHeaderReader<'r> {
    /// Calculates the hash for [self.as_slice()] as the pow hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawHeader, calc_pow_hash);

impl<'r> packed::HeaderReader<'r> {
    /// Calls [`RawHeaderReader.calc_pow_hash()`] for [`self.raw()`].
    ///
    /// [`RawHeaderReader.calc_pow_hash()`]: struct.RawHeaderReader.html#method.calc_pow_hash
    /// [`self.raw()`]: #method.raw
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.raw().calc_pow_hash()
    }

    /// Calculates the hash for [self.as_slice()] as the header hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Header, calc_pow_hash);
impl_calc_special_hash_for_entity!(Header, calc_header_hash);

impl<'r> packed::UncleBlockReader<'r> {
    /// Calls [`HeaderReader.calc_header_hash()`] for [`self.header()`].
    ///
    /// [`HeaderReader.calc_header_hash()`]: struct.HeaderReader.html#method.calc_header_hash
    /// [`self.header()`]: #method.header
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    /// Calls [`ProposalShortIdVecReader.calc_proposals_hash()`] for [`self.proposals()`].
    ///
    /// [`ProposalShortIdVecReader.calc_proposals_hash()`]: struct.ProposalShortIdVecReader.html#method.calc_proposals_hash
    /// [`self.proposals()`]: #method.proposals
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }
}
impl_calc_special_hash_for_entity!(UncleBlock, calc_header_hash);
impl_calc_special_hash_for_entity!(UncleBlock, calc_proposals_hash);

impl<'r> packed::UncleBlockVecReader<'r> {
    /// Calculates the hash for uncle blocks.
    ///
    /// Returns the empty hash if no uncle block, otherwise, calculates a hash for all header
    /// hashes of uncle blocks and returns it.
    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        if self.is_empty() {
            packed::Byte32::zero()
        } else {
            let mut ret = [0u8; 32];
            let mut blake2b = new_blake2b();
            for uncle in self.iter() {
                blake2b.update(uncle.calc_header_hash().as_slice());
            }
            blake2b.finalize(&mut ret);
            ret.pack()
        }
    }
}
impl_calc_special_hash_for_entity!(UncleBlockVec, calc_uncles_hash);

impl<'r> packed::BlockReader<'r> {
    /// Calls [`HeaderReader.calc_header_hash()`] for [`self.header()`].
    ///
    /// [`HeaderReader.calc_header_hash()`]: struct.HeaderReader.html#method.calc_header_hash
    /// [`self.header()`]: #method.header
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    /// Calls [`ProposalShortIdVecReader.calc_proposals_hash()`] for [`self.proposals()`].
    ///
    /// [`ProposalShortIdVecReader.calc_proposals_hash()`]: struct.ProposalShortIdVecReader.html#method.calc_proposals_hash
    /// [`self.proposals()`]: #method.proposals
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }

    /// Calls [`UncleBlockVecReader.calc_uncles_hash()`] for [`self.uncles()`].
    ///
    /// [`UncleBlockVecReader.calc_uncles_hash()`]: struct.UncleBlockVecReader.html#method.calc_uncles_hash
    /// [`self.uncles()`]: #method.uncles
    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        self.uncles().calc_uncles_hash()
    }

    /// Calculates the hash for the extension.
    ///
    /// If there is an extension (unknown for now), calculate the hash of its data.
    pub fn calc_extension_hash(&self) -> Option<packed::Byte32> {
        self.extension()
            .map(|extension| extension.calc_raw_data_hash())
    }

    /// Calculates the extra hash, which is a combination of the uncles hash and
    /// the extension hash.
    ///
    /// - If there is no extension, extra hash is the same as the uncles hash.
    /// - If there is a extension, then extra hash it the hash of the combination
    /// of uncles hash and the extension hash.
    pub fn calc_extra_hash(&self) -> core::ExtraHashView {
        core::ExtraHashView::new(self.calc_uncles_hash(), self.calc_extension_hash())
    }

    /// Calculates transaction hashes for all transactions in the block.
    pub fn calc_tx_hashes(&self) -> Vec<packed::Byte32> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_tx_hash())
            .collect::<Vec<_>>()
    }

    /// Calculates transaction witness hashes for all transactions in the block.
    pub fn calc_tx_witness_hashes(&self) -> Vec<packed::Byte32> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_witness_hash())
            .collect::<Vec<_>>()
    }
}

impl_calc_special_hash_for_entity!(Block, calc_header_hash);
impl_calc_special_hash_for_entity!(Block, calc_proposals_hash);
impl_calc_special_hash_for_entity!(Block, calc_uncles_hash);
impl_calc_special_hash_for_entity!(Block, calc_extension_hash, Option<packed::Byte32>);
impl_calc_special_hash_for_entity!(Block, calc_extra_hash, core::ExtraHashView);
impl_calc_special_hash_for_entity!(Block, calc_tx_hashes, Vec<packed::Byte32>);
impl_calc_special_hash_for_entity!(Block, calc_tx_witness_hashes, Vec<packed::Byte32>);

impl<'r> packed::CompactBlockReader<'r> {
    /// Calls [`HeaderReader.calc_header_hash()`] for [`self.header()`].
    ///
    /// [`HeaderReader.calc_header_hash()`]: struct.HeaderReader.html#method.calc_header_hash
    /// [`self.header()`]: #method.header
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }
}
impl_calc_special_hash_for_entity!(CompactBlock, calc_header_hash);

impl<'r> packed::RawAlertReader<'r> {
    /// Calculates the hash for [self.as_slice()] as the alert hash.
    ///
    /// [self.as_slice()]: ../prelude/trait.Reader.html#tymethod.as_slice
    pub fn calc_alert_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawAlert, calc_alert_hash);

impl<'r> packed::AlertReader<'r> {
    /// Calls [`RawAlertReader.calc_alert_hash()`] for [`self.raw()`].
    ///
    /// [`RawAlertReader.calc_alert_hash()`]: struct.RawAlertReader.html#method.calc_alert_hash
    /// [`self.raw()`]: #method.raw
    pub fn calc_alert_hash(&self) -> packed::Byte32 {
        self.raw().calc_alert_hash()
    }
}
impl_calc_special_hash_for_entity!(Alert, calc_alert_hash);
