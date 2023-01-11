use ckb_systemtime::unix_time;
use ckb_types::core::service::PoolTransactionEntry;
use ckb_types::{
    core::{tx_pool::Reject, BlockView},
    packed,
};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct Block {
    hash: packed::Byte32,
    number: u64,
    timestamp: u64,
    transactions: Vec<packed::Byte32>,
    seen_dt: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct Transaction {
    hash: packed::Byte32,
    cycles: u64,
    size: u64,
    fee: u64,
    seen_dt: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectedType {
    Invalid,
    Exceeded,
}

#[derive(Debug, Clone)]
pub(crate) struct RejectedTransaction {
    transaction: Transaction,
    rejected_type: RejectedType,
    rejected_desc: String,
}

impl From<BlockView> for Block {
    fn from(block: BlockView) -> Self {
        let hash = block.hash();
        let number = block.number();
        let timestamp = block.timestamp();
        let transactions: Vec<packed::Byte32> = block.tx_hashes().to_owned();
        let seen_dt = unix_time();
        Self {
            hash,
            number,
            timestamp,
            transactions,
            seen_dt,
        }
    }
}

impl Block {
    pub(crate) fn hash(&self) -> packed::Byte32 {
        self.hash.clone()
    }

    pub(crate) fn number(&self) -> u64 {
        self.number
    }

    pub(crate) fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub(crate) fn tx_hashes(&self) -> &[packed::Byte32] {
        &self.transactions[..]
    }

    pub(crate) fn seen_dt(&self) -> Duration {
        self.seen_dt
    }
}

impl From<PoolTransactionEntry> for Transaction {
    fn from(entry: PoolTransactionEntry) -> Self {
        let hash: packed::Byte32 = entry.transaction.hash();
        let cycles: u64 = entry.cycles;
        let size: u64 = entry.size as u64;
        let fee: u64 = entry.fee.as_u64();
        let seen_dt = unix_time();
        Self {
            hash,
            cycles,
            size,
            fee,
            seen_dt,
        }
    }
}

impl Transaction {
    pub(crate) fn hash(&self) -> packed::Byte32 {
        self.hash.clone()
    }

    pub(crate) fn cycles(&self) -> u64 {
        self.cycles
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn fee(&self) -> u64 {
        self.fee
    }

    pub(crate) fn seen_dt(&self) -> Duration {
        self.seen_dt
    }
}

impl From<(PoolTransactionEntry, Reject)> for RejectedTransaction {
    fn from((entry, reject): (PoolTransactionEntry, Reject)) -> Self {
        let (rejected_type, rejected_desc) = match reject {
            Reject::LowFeeRate(..) => (RejectedType::Exceeded, format!("{}", reject)),
            Reject::ExceededMaximumAncestorsCount => (RejectedType::Invalid, format!("{}", reject)),
            Reject::Full(..) => (RejectedType::Exceeded, format!("{}", reject)),
            Reject::Duplicated(_) => (RejectedType::Exceeded, format!("{}", reject)),
            Reject::Malformed(_) => (RejectedType::Invalid, format!("{}", reject)),
            Reject::DeclaredWrongCycles(..) => (RejectedType::Invalid, format!("{}", reject)),
            Reject::Resolve(_) => (RejectedType::Invalid, format!("{}", reject)),
            Reject::Verification(_) => (RejectedType::Invalid, format!("{}", reject)),
            Reject::Expiry(_) => (RejectedType::Exceeded, format!("{}", reject)),
            Reject::ExceededTransactionSizeLimit(..) => {
                (RejectedType::Exceeded, format!("{}", reject))
            }
        };
        Self {
            transaction: entry.into(),
            rejected_type,
            rejected_desc,
        }
    }
}

impl RejectedTransaction {
    pub(crate) fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    pub(crate) fn is_invalid(&self) -> bool {
        self.rejected_type == RejectedType::Invalid
    }

    pub(crate) fn reason(&self) -> &str {
        &self.rejected_desc
    }

    pub(crate) fn hash(&self) -> packed::Byte32 {
        self.transaction().hash()
    }
}
