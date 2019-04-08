use bit_vec::BitVec;
use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(remote = "BitVec")]
struct BitVecSerde {
    #[serde(getter = "BitVec::to_bytes")]
    bits: Vec<u8>,
}

impl From<BitVecSerde> for BitVec {
    fn from(bv: BitVecSerde) -> BitVec {
        BitVec::from_bytes(&bv.bits)
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct TransactionMeta {
    block_number: u64,
    /// first bit indicate if transaction is a cellbase transaction
    /// next bits indicate if transaction has deal cells
    #[serde(with = "BitVecSerde")]
    dead_cell: BitVec,
}

impl TransactionMeta {
    pub fn new(block_number: u64, outputs_count: usize) -> TransactionMeta {
        TransactionMeta {
            block_number,
            dead_cell: BitVec::from_elem(outputs_count + 1, false),
        }
    }

    /// New cellbase transaction
    pub fn new_cellbase(block_number: u64, outputs: usize) -> Self {
        let mut result = Self::new(block_number, outputs);
        result.dead_cell.set(0, true);
        result
    }

    /// Returns true if it is a cellbase transaction
    pub fn is_cellbase(&self) -> bool {
        self.dead_cell.get(0).expect("One bit should always exists")
    }

    pub fn len(&self) -> usize {
        self.dead_cell.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dead_cell.is_empty()
    }

    pub fn is_new(&self) -> bool {
        self.dead_cell.none()
    }

    pub fn is_all_dead(&self) -> bool {
        self.dead_cell.all()
    }

    pub fn is_dead(&self, index: usize) -> bool {
        self.dead_cell.get(index + 1).unwrap_or(true)
    }

    pub fn set_dead(&mut self, index: usize) {
        self.dead_cell.set(index + 1, true);
    }

    pub fn unset_dead(&mut self, index: usize) {
        self.dead_cell.set(index + 1, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode;

    #[test]
    fn transaction_meta_serde() {
        let mut original = TransactionMeta::new(0, 4);
        original.set_dead(1);
        original.set_dead(3);

        let decoded: TransactionMeta =
            bincode::deserialize(&(bincode::serialize(&original).unwrap())[..]).unwrap();

        assert!(!decoded.is_dead(0));
        assert!(decoded.is_dead(1));
        assert!(!decoded.is_dead(2));
        assert!(decoded.is_dead(3));
    }
}
