pub mod commit_txs_scanner;
pub mod entry;

pub(crate) mod container;
pub(crate) mod orphan;
pub(crate) mod pending;
pub(crate) mod proposed;

pub use self::entry::{DefectEntry, TxEntry};
pub(crate) use self::{
    container::{AncestorsScoreSortKey, SortedTxMap, TxLink},
    orphan::OrphanPool,
    pending::PendingQueue,
    proposed::{Edges, ProposedPool},
};
pub(crate) use ckb_verification::cache::CacheEntry;

/// Equal to MAX_BLOCK_BYTES / MAX_BLOCK_CYCLES, see ckb-chain-spec.
/// The precision is set so that the difference between MAX_BLOCK_CYCLES * DEFAULT_BYTES_PER_CYCLES
/// and MAX_BLOCK_BYTES is less than 1.
const DEFAULT_BYTES_PER_CYCLES: f64 = 0.000_170_571_4_f64;

/// Virtual bytes(aka vbytes) is a concept to unify the size and cycles of a transaction,
/// tx_pool use vbytes to estimate transaction fee rate.
pub(crate) fn get_transaction_virtual_bytes(tx_size: usize, cycles: u64) -> u64 {
    std::cmp::max(
        tx_size as u64,
        (cycles as f64 * DEFAULT_BYTES_PER_CYCLES) as u64,
    )
}
