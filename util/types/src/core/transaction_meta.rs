use crate::packed::Byte32;
use bit_vec::BitVec;

/// Metadata for tracking transaction state in the chain.
///
/// Stores information about which outputs of a transaction have been spent.
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
    /// Creates new transaction metadata.
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

    /// Returns the block number where this transaction was included.
    pub fn block_number(&self) -> u64 {
        self.block_number
    }

    /// Returns the epoch number where this transaction was included.
    pub fn epoch_number(&self) -> u64 {
        self.epoch_number
    }

    /// Returns the hash of the block containing this transaction.
    pub fn block_hash(&self) -> Byte32 {
        self.block_hash.clone()
    }

    /// Returns true if the transaction has no outputs.
    pub fn is_empty(&self) -> bool {
        self.dead_cell.is_empty()
    }

    /// Returns whether the output at the given index has been spent.
    pub fn is_dead(&self, index: usize) -> Option<bool> {
        self.dead_cell.get(index)
    }

    /// Returns true if all outputs have been spent.
    pub fn all_dead(&self) -> bool {
        self.dead_cell.all()
    }

    /// Marks the output at the given index as spent.
    pub fn set_dead(&mut self, index: usize) {
        if index < self.len() {
            self.dead_cell.set(index, true);
        }
    }

    /// Marks the output at the given index as unspent.
    pub fn unset_dead(&mut self, index: usize) {
        if index < self.len() {
            self.dead_cell.set(index, false);
        }
    }
}

/// Builder for `TransactionMeta`.
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
    /// Sets the block number.
    pub fn block_number(mut self, block_number: u64) -> Self {
        self.block_number = block_number;
        self
    }

    /// Sets the epoch number.
    pub fn epoch_number(mut self, epoch_number: u64) -> Self {
        self.epoch_number = epoch_number;
        self
    }

    /// Sets the block hash.
    pub fn block_hash(mut self, block_hash: Byte32) -> Self {
        self.block_hash = block_hash;
        self
    }

    /// Sets whether this is a cellbase transaction.
    pub fn cellbase(mut self, cellbase: bool) -> Self {
        self.cellbase = cellbase;
        self
    }

    /// Sets the bit vector indicating spent outputs.
    pub fn bits(mut self, bits: Vec<u8>) -> Self {
        self.bits = bits;
        self
    }

    /// Sets the total number of outputs.
    pub fn len(mut self, len: usize) -> Self {
        self.len = len;
        self
    }

    /// Builds the `TransactionMeta`.
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
