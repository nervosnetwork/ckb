use std::u64::MAX;

#[derive(Default, Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct TransactionMeta {
    /// Height of the last block containing the transaction.
    pub height: u64,

    /// At which height the output is spent.
    /// TODO: Use bitvec to save storage space?
    /// u64 MAX indicates not spent yet.
    pub spent_at: Vec<u64>,
}

impl TransactionMeta {
    pub fn new(height: u64, outputs_count: usize) -> TransactionMeta {
        TransactionMeta {
            height,
            spent_at: vec![MAX; outputs_count],
        }
    }

    pub fn is_fully_spent(&self) -> bool {
        self.spent_at.iter().all(|&x| x != MAX)
    }

    pub fn is_spent(&self, index: usize) -> bool {
        self.spent_at[index] != MAX
    }

    pub fn set_spent(&mut self, index: usize, height: u64) {
        self.spent_at[index] = height;
    }

    pub fn unset_spent(&mut self, index: usize) {
        self.spent_at[index] = MAX;
    }
}
