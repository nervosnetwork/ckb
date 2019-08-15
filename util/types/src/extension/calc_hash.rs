use ckb_hash::{blake2b_256, new_blake2b};

use crate::{packed, prelude::*, H256};

macro_rules! impl_calc_hash_for_reader {
    ($reader:ident) => {
        impl<'r> CalcHash for packed::$reader<'r> {
            fn calc_hash(&self) -> H256 {
                blake2b_256(self.as_slice()).into()
            }
        }
    };
}

macro_rules! impl_calc_hash_for_entity {
    ($entity:ident) => {
        impl CalcHash for packed::$entity {
            fn calc_hash(&self) -> H256 {
                self.as_reader().calc_hash()
            }
        }
    };
}

macro_rules! impl_calc_hash_for_both {
    ($entity:ident, $reader:ident) => {
        impl_calc_hash_for_reader!($reader);
        impl_calc_hash_for_entity!($entity);
    };
}

macro_rules! impl_calc_special_hash_for_entity {
    ($entity:ident, $func_name:ident) => {
        impl packed::$entity {
            pub fn $func_name(&self) -> H256 {
                self.as_reader().$func_name()
            }
        }
    };
}

impl_calc_hash_for_both!(RawHeader, RawHeaderReader);
impl_calc_hash_for_both!(Header, HeaderReader);
impl_calc_hash_for_both!(RawTransaction, RawTransactionReader);
impl_calc_hash_for_both!(SlimTransaction, SlimTransactionReader);
impl_calc_hash_for_both!(Script, ScriptReader);
impl_calc_hash_for_both!(RawAlert, RawAlertReader);

impl<'r> CalcHash for packed::UncleBlockVecReader<'r> {
    fn calc_hash(&self) -> H256 {
        if self.is_empty() {
            H256::zero()
        } else {
            blake2b_256(self.as_slice()).into()
        }
    }
}
impl_calc_hash_for_entity!(UncleBlockVec);

impl<'r> CalcHash for packed::ProposalShortIdVecReader<'r> {
    fn calc_hash(&self) -> H256 {
        if self.is_empty() {
            H256::zero()
        } else {
            let mut ret = [0u8; 32];
            let mut blake2b = new_blake2b();
            for id in self.iter() {
                blake2b.update(id.as_slice());
            }
            blake2b.finalize(&mut ret);
            ret.into()
        }
    }
}
impl_calc_hash_for_entity!(ProposalShortIdVec);

impl<'r> packed::TransactionReader<'r> {
    pub fn calc_tx_hash(&self) -> H256 {
        self.slim().raw().calc_hash()
    }

    pub fn calc_witness_hash(&self) -> H256 {
        self.slim().calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Transaction, calc_tx_hash);
impl_calc_special_hash_for_entity!(Transaction, calc_witness_hash);

impl<'r> packed::HeaderReader<'r> {
    pub fn calc_pow_hash(&self) -> H256 {
        self.raw().calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Header, calc_pow_hash);

impl<'r> packed::UncleBlockReader<'r> {
    pub fn calc_header_hash(&self) -> H256 {
        self.header().calc_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.proposals().calc_hash()
    }
}
impl_calc_special_hash_for_entity!(UncleBlock, calc_header_hash);
impl_calc_special_hash_for_entity!(UncleBlock, calc_proposals_hash);

impl<'r> packed::BlockReader<'r> {
    pub fn calc_header_hash(&self) -> H256 {
        self.header().calc_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.proposals().calc_hash()
    }

    pub fn calc_uncles_hash(&self) -> H256 {
        self.uncles().calc_hash()
    }

    pub fn calc_tx_hashes(&self) -> Vec<H256> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_tx_hash())
            .collect::<Vec<_>>()
    }

    pub fn calc_tx_witness_hashes(&self) -> Vec<H256> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_witness_hash())
            .collect::<Vec<_>>()
    }
}

impl_calc_special_hash_for_entity!(Block, calc_header_hash);
impl_calc_special_hash_for_entity!(Block, calc_proposals_hash);
impl_calc_special_hash_for_entity!(Block, calc_uncles_hash);

impl packed::CellOutput {
    pub fn calc_data_hash(data: &[u8]) -> H256 {
        if data.is_empty() {
            H256::zero()
        } else {
            blake2b_256(data).into()
        }
    }
}
