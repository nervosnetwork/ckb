use ckb_types::core::{BlockView, TransactionView};
use ckb_types::packed::{Block, Byte32, Transaction};

pub trait GetCommitTxId {
    fn get_commit_tx_id(&self) -> Byte32;
}

impl GetCommitTxId for TransactionView {
    fn get_commit_tx_id(&self) -> Byte32 {
        self.hash()
    }
}

impl GetCommitTxId for Transaction {
    fn get_commit_tx_id(&self) -> Byte32 {
        self.calc_tx_hash()
    }
}

pub trait GetCommitTxIds {
    fn get_commit_tx_ids(&self) -> Vec<Byte32>;
}

impl<T> GetCommitTxIds for T
where
    T: GetCommitTxId,
{
    fn get_commit_tx_ids(&self) -> Vec<Byte32> {
        vec![self.get_commit_tx_id()]
    }
}

impl<T> GetCommitTxIds for Vec<T>
where
    T: GetCommitTxId,
{
    fn get_commit_tx_ids(&self) -> Vec<Byte32> {
        self.iter().map(|t| t.get_commit_tx_id()).collect()
    }
}

impl GetCommitTxIds for Block {
    fn get_commit_tx_ids(&self) -> Vec<Byte32> {
        self.transactions()
            .into_iter()
            .skip(1)
            .map(|tx| tx.calc_tx_hash())
            .collect()
    }
}

impl GetCommitTxIds for BlockView {
    fn get_commit_tx_ids(&self) -> Vec<Byte32> {
        self.transactions()
            .iter()
            .skip(1)
            .map(|tx| tx.hash())
            .collect()
    }
}
