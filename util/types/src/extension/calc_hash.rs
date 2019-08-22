use ckb_hash::{blake2b_256, new_blake2b};

use crate::{packed, prelude::*, H256};

/*
 * Calculate simple hash for packed bytes wrappers.
 */

// Do NOT use this trait directly.
// Please call the methods which specify the hash content.
pub(crate) trait CalcHash {
    fn calc_hash(&self) -> H256;
}

impl<'r, R> CalcHash for R
where
    R: Reader<'r>,
{
    fn calc_hash(&self) -> H256 {
        blake2b_256(self.as_slice()).into()
    }
}

impl packed::CellOutput {
    pub fn calc_data_hash(data: &[u8]) -> H256 {
        if data.is_empty() {
            H256::zero()
        } else {
            blake2b_256(data).into()
        }
    }
}

impl<'r> packed::ScriptReader<'r> {
    pub fn calc_script_hash(&self) -> H256 {
        self.calc_hash()
    }
}

impl packed::Script {
    pub fn calc_script_hash(&self) -> H256 {
        self.as_reader().calc_script_hash()
    }
}

impl<'r> packed::CellOutputReader<'r> {
    pub fn calc_lock_hash(&self) -> H256 {
        self.lock().calc_script_hash()
    }
}

impl packed::CellOutput {
    pub fn calc_lock_hash(&self) -> H256 {
        self.as_reader().calc_lock_hash()
    }
}

impl<'r> packed::ProposalShortIdVecReader<'r> {
    pub fn calc_proposals_hash(&self) -> H256 {
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

impl packed::ProposalShortIdVec {
    pub fn calc_proposals_hash(&self) -> H256 {
        self.as_reader().calc_proposals_hash()
    }
}

impl<'r> packed::TransactionReader<'r> {
    pub fn calc_tx_hash(&self) -> H256 {
        self.slim().raw().calc_hash()
    }

    pub fn calc_witness_hash(&self) -> H256 {
        self.slim().calc_hash()
    }
}

impl packed::Transaction {
    pub fn calc_tx_hash(&self) -> H256 {
        self.as_reader().calc_tx_hash()
    }

    pub fn calc_witness_hash(&self) -> H256 {
        self.as_reader().calc_witness_hash()
    }
}

impl<'r> packed::RawHeaderReader<'r> {
    pub fn calc_pow_hash(&self) -> H256 {
        self.calc_hash()
    }
}

impl packed::RawHeader {
    pub fn calc_pow_hash(&self) -> H256 {
        self.as_reader().calc_pow_hash()
    }
}

impl<'r> packed::HeaderReader<'r> {
    pub fn calc_pow_hash(&self) -> H256 {
        self.raw().calc_pow_hash()
    }

    pub fn calc_header_hash(&self) -> H256 {
        self.calc_hash()
    }
}

impl packed::Header {
    pub fn calc_pow_hash(&self) -> H256 {
        self.as_reader().calc_pow_hash()
    }

    pub fn calc_header_hash(&self) -> H256 {
        self.as_reader().calc_header_hash()
    }
}

impl<'r> packed::UncleBlockReader<'r> {
    pub fn calc_header_hash(&self) -> H256 {
        self.header().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.proposals().calc_proposals_hash()
    }
}

impl packed::UncleBlock {
    pub fn calc_header_hash(&self) -> H256 {
        self.as_reader().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.as_reader().calc_proposals_hash()
    }
}

impl<'r> packed::UncleBlockVecReader<'r> {
    pub fn calc_uncles_hash(&self) -> H256 {
        if self.is_empty() {
            H256::zero()
        } else {
            blake2b_256(self.as_slice()).into()
        }
    }
}

impl packed::UncleBlockVec {
    pub fn calc_uncles_hash(&self) -> H256 {
        self.as_reader().calc_uncles_hash()
    }
}

impl<'r> packed::BlockReader<'r> {
    pub fn calc_header_hash(&self) -> H256 {
        self.header().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.proposals().calc_proposals_hash()
    }

    pub fn calc_uncles_hash(&self) -> H256 {
        self.uncles().calc_uncles_hash()
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

impl packed::Block {
    pub fn calc_header_hash(&self) -> H256 {
        self.as_reader().calc_header_hash()
    }

    pub fn calc_proposals_hash(&self) -> H256 {
        self.as_reader().calc_proposals_hash()
    }

    pub fn calc_uncles_hash(&self) -> H256 {
        self.as_reader().calc_uncles_hash()
    }
}

impl packed::Block {
    pub fn calc_tx_hashes(&self) -> Vec<H256> {
        self.as_reader().calc_tx_hashes()
    }

    pub fn calc_tx_witness_hashes(&self) -> Vec<H256> {
        self.as_reader().calc_tx_witness_hashes()
    }
}

impl<'r> packed::CompactBlockReader<'r> {
    pub fn calc_header_hash(&self) -> H256 {
        self.header().calc_header_hash()
    }
}

impl packed::CompactBlock {
    pub fn calc_header_hash(&self) -> H256 {
        self.as_reader().calc_header_hash()
    }
}

impl<'r> packed::RawAlertReader<'r> {
    pub fn calc_alert_hash(&self) -> H256 {
        self.calc_hash()
    }
}

impl packed::RawAlert {
    pub fn calc_alert_hash(&self) -> H256 {
        self.as_reader().calc_alert_hash()
    }
}

impl<'r> packed::AlertReader<'r> {
    pub fn calc_alert_hash(&self) -> H256 {
        self.raw().calc_alert_hash()
    }
}

impl packed::Alert {
    pub fn calc_alert_hash(&self) -> H256 {
        self.as_reader().calc_alert_hash()
    }
}
