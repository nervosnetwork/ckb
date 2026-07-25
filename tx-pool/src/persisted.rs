use crate::TxPool;
use ckb_error::{AnyError, OtherError};
use ckb_types::{
    core::TransactionView,
    packed::{TransactionVec, TransactionVecReader},
    prelude::*,
};
use std::{
    fs::OpenOptions,
    io::{Read as _, Write as _},
};

/// The version of the persisted tx-pool data.
pub(crate) const VERSION: u32 = 1;

impl TxPool {
    pub(crate) fn load_from_file(&self) -> Result<Vec<TransactionView>, AnyError> {
        let mut persisted_data_file = self.config.persisted_data.clone();
        persisted_data_file.set_extension(format!("v{VERSION}"));

        // Remove any stale temporary file left by a previous interrupted
        // persistence attempt. The final file, if it exists, is authoritative.
        let tmp_file = persisted_data_file.with_extension(format!("v{VERSION}.tmp"));
        let _ = std::fs::remove_file(&tmp_file);

        if persisted_data_file.exists() {
            let mut file = OpenOptions::new()
                .read(true)
                .open(&persisted_data_file)
                .map_err(|err| {
                    let errmsg = format!(
                        "Failed to open the tx-pool persisted data file [{persisted_data_file:?}], cause: {err}"
                    );
                    OtherError::new(errmsg)
                })?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|err| {
                let errmsg = format!(
                    "Failed to read the tx-pool persisted data file [{persisted_data_file:?}], cause: {err}"
                );
                OtherError::new(errmsg)
            })?;

            let persisted_data = TransactionVecReader::from_slice(&buffer)
                .map_err(|err| {
                    let errmsg = format!(
                        "The tx-pool persisted data file [{persisted_data_file:?}] is broken, cause: {err}"
                    );
                    OtherError::new(errmsg)
                })?
                .to_entity();

            Ok(persisted_data
                .into_iter()
                .map(|tx| tx.into_view())
                .collect())
        } else {
            Ok(Vec::new())
        }
    }

    /// Persist the current in-memory pool to disk.
    ///
    /// This function performs blocking file I/O. It is intended to be called
    /// during shutdown while the tx-pool write lock is held, so no new
    /// transactions can enter the pool between collecting the transactions and
    /// draining them.
    pub(crate) fn save_into_file(&mut self) -> Result<(), AnyError> {
        let mut persisted_data_file = self.config.persisted_data.clone();
        persisted_data_file.set_extension(format!("v{VERSION}"));

        // Step 1: Collect transactions WITHOUT draining the pool.
        // If anything fails below, the in-memory pool remains intact.
        // `get_all_txs` uses the accepted PoolMap graph, including verified
        // dep-group expansion. Do not pass this through the raw-transaction
        // sorter: because that sorter cannot see expanded members, it can
        // legally move a child ahead of one of its authoritative parents.
        let all_txs = self.get_all_txs();
        let txs = TransactionVec::new_builder()
            .extend(all_txs.iter().map(|tx| tx.data()))
            .build();

        // Step 2: Write to a temporary file first.
        let tmp_file = persisted_data_file.with_extension(format!("v{VERSION}.tmp"));
        let write_result = (|| -> Result<(), AnyError> {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_file)
                .map_err(|err| {
                    let errmsg = format!(
                        "Failed to open temp file [{tmp_file:?}] for tx-pool persistence, cause: {err}"
                    );
                    OtherError::new(errmsg)
                })?;

            file.write_all(txs.as_slice()).map_err(|err| {
                let errmsg = format!(
                    "Failed to write tx-pool data into temp file [{tmp_file:?}], cause: {err}"
                );
                OtherError::new(errmsg)
            })?;

            file.sync_all().map_err(|err| {
                let errmsg = format!("Failed to sync temp file [{tmp_file:?}], cause: {err}");
                OtherError::new(errmsg)
            })?;
            drop(file);

            // Step 3: Rename the temporary file to the final name. On the same
            // filesystem this is typically atomic; cross-volume renames are not
            // handled here and will return an error.
            std::fs::rename(&tmp_file, &persisted_data_file).map_err(|err| {
                let errmsg = format!(
                    "Failed to rename temp file [{tmp_file:?}] to [{persisted_data_file:?}], cause: {err}"
                );
                OtherError::new(errmsg)
            })?;
            Ok(())
        })();

        if write_result.is_err() {
            // Best-effort cleanup of the incomplete temporary file. Ignore
            // removal errors: the file may not exist or may be unreadable.
            let _ = std::fs::remove_file(&tmp_file);
            return write_result;
        }

        // Step 4: Only after successful persistence, drop the in-memory pool.
        // The transactions were already collected in step 1, so there is no
        // need to run a full `TxSelector` pass here just to discard the result.
        // Use the full clear (pool map plus committed/conflicts caches) so the
        // state afterwards is identical to `clear_pool`: leftover conflict
        // entries could otherwise "resurrect" transactions that were just
        // dumped to disk once their inputs become available again.
        let snapshot = self.cloned_snapshot();
        self.clear(snapshot);

        Ok(())
    }
}
