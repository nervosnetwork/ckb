use ckb_hash::{blake2b_256, new_blake2b};

use crate::{packed, prelude::*};

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
    ($entity:ident, $func_name:ident) => {
        impl packed::$entity {
            pub fn $func_name(&self) -> packed::Byte32 {
                self.as_reader().$func_name()
            }
        }
    };
}

impl packed::CellOutput {
    pub fn calc_data_hash(data: &[u8]) -> packed::Byte32 {
        if data.is_empty() {
            packed::Byte32::zero()
        } else {
            blake2b_256(data).pack()
        }
    }
}

impl<'r> packed::ScriptReader<'r> {
    pub fn calc_script_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Script, calc_script_hash);

impl<'r> packed::CellOutputReader<'r> {
    pub fn calc_lock_hash(&self) -> packed::Byte32 {
        self.lock().calc_script_hash()
    }
}
impl_calc_special_hash_for_entity!(CellOutput, calc_lock_hash);

impl<'r> packed::ProposalShortIdVecReader<'r> {
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

impl<'r> packed::TransactionReader<'r> {
    pub fn calc_tx_hash(&self) -> packed::Byte32 {
        self.raw().calc_hash()
    }

    pub fn calc_witness_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Transaction, calc_tx_hash);
impl_calc_special_hash_for_entity!(Transaction, calc_witness_hash);

impl<'r> packed::RawHeaderReader<'r> {
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawHeader, calc_pow_hash);

impl<'r> packed::HeaderReader<'r> {
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.raw().calc_pow_hash()
    }

    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Header, calc_pow_hash);
impl_calc_special_hash_for_entity!(Header, calc_header_hash);

impl<'r> packed::UncleBlockReader<'r> {
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }
}
impl_calc_special_hash_for_entity!(UncleBlock, calc_header_hash);
impl_calc_special_hash_for_entity!(UncleBlock, calc_proposals_hash);

impl<'r> packed::UncleBlockVecReader<'r> {
    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        if self.is_empty() {
            packed::Byte32::zero()
        } else {
            blake2b_256(self.as_slice()).pack()
        }
    }
}
impl_calc_special_hash_for_entity!(UncleBlockVec, calc_uncles_hash);

impl<'r> packed::BlockReader<'r> {
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }

    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        self.uncles().calc_uncles_hash()
    }

    pub fn calc_tx_hashes(&self) -> Vec<packed::Byte32> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_tx_hash())
            .collect::<Vec<_>>()
    }

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

impl packed::Block {
    pub fn calc_tx_hashes(&self) -> Vec<packed::Byte32> {
        self.as_reader().calc_tx_hashes()
    }

    pub fn calc_tx_witness_hashes(&self) -> Vec<packed::Byte32> {
        self.as_reader().calc_tx_witness_hashes()
    }
}

impl<'r> packed::CompactBlockReader<'r> {
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }
}
impl_calc_special_hash_for_entity!(CompactBlock, calc_header_hash);

impl<'r> packed::RawAlertReader<'r> {
    pub fn calc_alert_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawAlert, calc_alert_hash);

impl<'r> packed::AlertReader<'r> {
    pub fn calc_alert_hash(&self) -> packed::Byte32 {
        self.raw().calc_alert_hash()
    }
}
impl_calc_special_hash_for_entity!(Alert, calc_alert_hash);

#[cfg(test)]
mod tests {
    use crate::{h256, packed, prelude::*, H256};
    use ckb_hash::blake2b_256;

    #[test]
    fn proposals_hash() {
        let proposal1 = [1; 10].pack();
        let proposal2 = [2; 10].pack();
        let proposals = vec![proposal1, proposal2].pack();
        let expect = h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562");
        assert_eq!(proposals.calc_proposals_hash(), expect.pack());
    }

    #[test]
    fn empty_proposals_hash() {
        let proposals = packed::ProposalShortIdVec::new_builder().build();
        let expect = h256!("0x0");
        assert_eq!(proposals.calc_proposals_hash(), expect.pack());
    }

    #[test]
    fn empty_script_hash() {
        let script = packed::Script::new_builder().build();
        let expect = h256!("0xbd7e6000ffb8e983a6023809037e0c4cedbc983637c46d74621fd28e5f15fe4f");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../../../script/testdata/always_success");
        let always_success_hash = blake2b_256(&always_success[..]);

        let script = packed::Script::new_builder()
            .code_hash(always_success_hash.pack())
            .build();
        let expect = h256!("0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }

    #[test]
    fn one_arg_script_hash() {
        let script = packed::Script::new_builder().args(vec![1].pack()).build();
        let expect = h256!("0x5a2b913dfb1b79136fc72a575fd8e93ae080b504463c0066fea086482bfc3a94");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }
}
