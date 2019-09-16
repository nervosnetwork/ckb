use bit_vec::BitVec;

use crate::packed::Byte32;

#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct TransactionMeta {
    pub(crate) block_number: u64,
    pub(crate) epoch_number: u64,
    pub(crate) block_hash: Byte32,
    pub(crate) cellbase: bool,
    /// each bits indicate if transaction has dead cells
    pub(crate) dead_cell: BitVec,
}

impl TransactionMeta {
    pub fn new(
        block_number: u64,
        epoch_number: u64,
        block_hash: Byte32,
        outputs_count: usize,
        all_dead: bool,
    ) -> TransactionMeta {
        TransactionMeta {
            block_number,
            epoch_number,
            block_hash,
            cellbase: false,
            dead_cell: BitVec::from_elem(outputs_count, all_dead),
        }
    }

    /// New cellbase transaction
    pub fn new_cellbase(
        block_number: u64,
        epoch_number: u64,
        block_hash: Byte32,
        outputs_count: usize,
        all_dead: bool,
    ) -> Self {
        let mut result = Self::new(
            block_number,
            epoch_number,
            block_hash,
            outputs_count,
            all_dead,
        );
        result.cellbase = true;
        result
    }

    /// Returns true if it is a cellbase transaction
    pub fn is_cellbase(&self) -> bool {
        self.cellbase
    }

    /// Returns transaction outputs count
    pub fn len(&self) -> usize {
        self.dead_cell.len()
    }

    pub fn block_number(&self) -> u64 {
        self.block_number
    }

    pub fn epoch_number(&self) -> u64 {
        self.epoch_number
    }

    pub fn block_hash(&self) -> Byte32 {
        self.block_hash.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.dead_cell.is_empty()
    }

    pub fn is_dead(&self, index: usize) -> Option<bool> {
        self.dead_cell.get(index)
    }

    pub fn all_dead(&self) -> bool {
        self.dead_cell.all()
    }

    pub fn set_dead(&mut self, index: usize) {
        if index < self.len() {
            self.dead_cell.set(index, true);
        }
    }

    pub fn unset_dead(&mut self, index: usize) {
        if index < self.len() {
            self.dead_cell.set(index, false);
        }
    }
}

#[derive(Default)]
pub struct TransactionMetaBuilder {
    block_number: u64,
    epoch_number: u64,
    block_hash: Byte32,
    cellbase: bool,
    bits: Vec<u8>,
    len: usize,
}

impl TransactionMetaBuilder {
    pub fn block_number(mut self, block_number: u64) -> Self {
        self.block_number = block_number;
        self
    }

    pub fn epoch_number(mut self, epoch_number: u64) -> Self {
        self.epoch_number = epoch_number;
        self
    }

    pub fn block_hash(mut self, block_hash: Byte32) -> Self {
        self.block_hash = block_hash;
        self
    }

    pub fn cellbase(mut self, cellbase: bool) -> Self {
        self.cellbase = cellbase;
        self
    }

    pub fn bits(mut self, bits: Vec<u8>) -> Self {
        self.bits = bits;
        self
    }

    pub fn len(mut self, len: usize) -> Self {
        self.len = len;
        self
    }

    pub fn build(self) -> TransactionMeta {
        let TransactionMetaBuilder {
            block_number,
            epoch_number,
            block_hash,
            cellbase,
            bits,
            len,
        } = self;
        let mut dead_cell = BitVec::from_bytes(&bits);
        dead_cell.truncate(len);
        TransactionMeta {
            block_number,
            epoch_number,
            block_hash,
            cellbase,
            dead_cell,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_unset_dead_out_of_bounds() {
        let mut meta = TransactionMeta::new(0, 0, Byte32::zero(), 4, false);
        meta.set_dead(3);
        assert!(meta.is_dead(3) == Some(true));
        meta.unset_dead(3);
        assert!(meta.is_dead(3) == Some(false));
        // none-op
        meta.set_dead(4);
        assert!(meta.is_dead(4) == None);
        meta.unset_dead(4);
        assert!(meta.is_dead(4) == None);
    }
}
