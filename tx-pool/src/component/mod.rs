pub mod commit_txs_scanner;
pub mod entry;

pub(crate) mod container;
pub(crate) mod orphan;
pub(crate) mod pending;
pub(crate) mod proposed;

pub use self::entry::{DefectEntry, TxEntry};

const DEFAULT_BYTES_PER_CYCLES: f64 = 0.000_17f64;

/// Virtual bytes(aka vbytes) is a concept to unify the size and cycles of a transaction,
/// tx_pool use vbytes to estimate transaction fee rate.
pub(crate) fn get_transaction_virtual_bytes(tx_size: usize, cycles: u64) -> u64 {
    std::cmp::max(
        tx_size as u64,
        (cycles as f64 * DEFAULT_BYTES_PER_CYCLES) as u64,
    )
}
