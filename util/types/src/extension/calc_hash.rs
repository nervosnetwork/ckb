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
            /// TODO(doc): @yangby-cryptape
            pub fn $func_name(&self) -> packed::Byte32 {
                self.as_reader().$func_name()
            }
        }
    };
}

impl packed::CellOutput {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_data_hash(data: &[u8]) -> packed::Byte32 {
        if data.is_empty() {
            packed::Byte32::zero()
        } else {
            blake2b_256(data).pack()
        }
    }
}

impl<'r> packed::ScriptReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_script_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Script, calc_script_hash);

impl<'r> packed::CellOutputReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_lock_hash(&self) -> packed::Byte32 {
        self.lock().calc_script_hash()
    }
}
impl_calc_special_hash_for_entity!(CellOutput, calc_lock_hash);

impl<'r> packed::ProposalShortIdVecReader<'r> {
    /// TODO(doc): @yangby-cryptape
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
    /// TODO(doc): @yangby-cryptape
    pub fn calc_tx_hash(&self) -> packed::Byte32 {
        self.raw().calc_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_witness_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Transaction, calc_tx_hash);
impl_calc_special_hash_for_entity!(Transaction, calc_witness_hash);

impl<'r> packed::RawHeaderReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawHeader, calc_pow_hash);

impl<'r> packed::HeaderReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_pow_hash(&self) -> packed::Byte32 {
        self.raw().calc_pow_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(Header, calc_pow_hash);
impl_calc_special_hash_for_entity!(Header, calc_header_hash);

impl<'r> packed::UncleBlockReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }
}
impl_calc_special_hash_for_entity!(UncleBlock, calc_header_hash);
impl_calc_special_hash_for_entity!(UncleBlock, calc_proposals_hash);

impl<'r> packed::UncleBlockVecReader<'r> {
    /// TODO(doc): @yangby-cryptape
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
    /// TODO(doc): @yangby-cryptape
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_proposals_hash(&self) -> packed::Byte32 {
        self.proposals().calc_proposals_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_uncles_hash(&self) -> packed::Byte32 {
        self.uncles().calc_uncles_hash()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_tx_hashes(&self) -> Vec<packed::Byte32> {
        self.transactions()
            .iter()
            .map(|tx| tx.calc_tx_hash())
            .collect::<Vec<_>>()
    }

    /// TODO(doc): @yangby-cryptape
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
    /// TODO(doc): @yangby-cryptape
    pub fn calc_tx_hashes(&self) -> Vec<packed::Byte32> {
        self.as_reader().calc_tx_hashes()
    }

    /// TODO(doc): @yangby-cryptape
    pub fn calc_tx_witness_hashes(&self) -> Vec<packed::Byte32> {
        self.as_reader().calc_tx_witness_hashes()
    }
}

impl<'r> packed::CompactBlockReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_header_hash(&self) -> packed::Byte32 {
        self.header().calc_header_hash()
    }
}
impl_calc_special_hash_for_entity!(CompactBlock, calc_header_hash);

impl<'r> packed::RawAlertReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn calc_alert_hash(&self) -> packed::Byte32 {
        self.calc_hash()
    }
}
impl_calc_special_hash_for_entity!(RawAlert, calc_alert_hash);

impl<'r> packed::AlertReader<'r> {
    /// TODO(doc): @yangby-cryptape
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
    fn uncles_hash() {
        let uncle1_raw_header = packed::RawHeader::new_builder()
            .version(0u32.pack())
            .compact_target(0x1e08_3126u32.pack())
            .timestamp(0x5cd2_b117u64.pack())
            .number(0x400u64.pack())
            .epoch(0x0007_0800_1800_0001u64.pack())
            .parent_hash(
                h256!("0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831").pack(),
            )
            .transactions_root(
                h256!("0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f").pack(),
            )
            .proposals_hash(
                h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").pack(),
            )
            .uncles_hash(
                h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
            )
            .dao(h256!("0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000").pack())
            .build();
        let uncle1_header = packed::Header::new_builder()
            .raw(uncle1_raw_header)
            .nonce(0x5ff1_389a_f870_6543_11a2_bee6_1237u128.pack())
            .build();
        let uncle1_proposals = vec![[1; 10].pack(), [2; 10].pack()].pack();
        let uncle1 = packed::UncleBlock::new_builder()
            .header(uncle1_header)
            .proposals(uncle1_proposals)
            .build();

        let uncle2_raw_header = packed::RawHeader::new_builder()
            .version(0u32.pack())
            .compact_target(0x2001_0000u32.pack())
            .timestamp(0x5cd2_1a16u64.pack())
            .number(0x400u64.pack())
            .epoch(0x0007_0800_1800_0001u64.pack())
            .parent_hash(
                h256!("0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831").pack(),
            )
            .transactions_root(
                h256!("0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f").pack(),
            )
            .proposals_hash(
                h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
            )
            .uncles_hash(
                h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
            )
            .dao(h256!("0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000").pack())
            .build();
        let uncle2_header = packed::Header::new_builder()
            .raw(uncle2_raw_header)
            .nonce(0x2f39_2d41_cd70_024fu128.pack())
            .build();
        let uncle2 = packed::UncleBlock::new_builder()
            .header(uncle2_header)
            .build();

        let uncles = vec![uncle1, uncle2].pack();
        let expect = h256!("0x0135d01f169a870bd9c92b2b37aecfa0fbfb7c1862cc176e03bb525fab0649d9");
        assert_eq!(uncles.calc_uncles_hash(), expect.pack());
    }

    #[test]
    fn empty_uncles_hash() {
        let uncles = packed::UncleBlockVec::new_builder().build();
        let expect = h256!("0x0");
        assert_eq!(uncles.calc_uncles_hash(), expect.pack());
    }

    #[test]
    fn empty_script_hash() {
        let script = packed::Script::new_builder().build();
        let expect = h256!("0x77c93b0632b5b6c3ef922c5b7cea208fb0a7c427a13d50e13d3fefad17e0c590");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../../../script/testdata/always_success");
        let always_success_hash = blake2b_256(&always_success[..]);

        let script = packed::Script::new_builder()
            .code_hash(always_success_hash.pack())
            .build();
        let expect = h256!("0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }

    #[test]
    fn one_arg_script_hash() {
        let script = packed::Script::new_builder().args(vec![1].pack()).build();
        let expect = h256!("0x67951b34bce20cb71b7e235c1f8cda259628d99d94825bffe549c23b4dd2930f");
        assert_eq!(script.calc_script_hash(), expect.pack());
    }
}
