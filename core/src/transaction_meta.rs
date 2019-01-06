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
    #[serde(with = "BitVecSerde")]
    pub output_spent: BitVec,
}

impl TransactionMeta {
    pub fn new(outputs_count: usize) -> TransactionMeta {
        TransactionMeta {
            output_spent: BitVec::from_elem(outputs_count, false),
        }
    }

    pub fn len(&self) -> usize {
        self.output_spent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.output_spent.is_empty()
    }

    pub fn is_new(&self) -> bool {
        self.output_spent.none()
    }

    pub fn is_fully_spent(&self) -> bool {
        self.output_spent.all()
    }

    pub fn is_spent(&self, index: usize) -> bool {
        self.output_spent.get(index).unwrap_or(true)
    }

    pub fn set_spent(&mut self, index: usize) {
        self.output_spent.set(index, true);
    }

    pub fn unset_spent(&mut self, index: usize) {
        self.output_spent.set(index, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode;

    #[test]
    fn transaction_meta_serde() {
        let mut original = TransactionMeta::new(4);
        original.set_spent(1);
        original.set_spent(3);

        let decoded: TransactionMeta =
            bincode::deserialize(&(bincode::serialize(&original).unwrap())[..]).unwrap();

        assert!(!decoded.is_spent(0));
        assert!(decoded.is_spent(1));
        assert!(!decoded.is_spent(2));
        assert!(decoded.is_spent(3));
    }
}
