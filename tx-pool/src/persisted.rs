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
        persisted_data_file.set_extension(format!("v{}", VERSION));

        if persisted_data_file.exists() {
            let mut file = OpenOptions::new()
                .read(true)
                .open(&persisted_data_file)
                .map_err(|err| {
                    let errmsg = format!(
                        "Failed to open the tx-pool persisted data file [{:?}], cause: {}",
                        persisted_data_file, err
                    );
                    OtherError::new(errmsg)
                })?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|err| {
                let errmsg = format!(
                    "Failed to read the tx-pool persisted data file [{:?}], cause: {}",
                    persisted_data_file, err
                );
                OtherError::new(errmsg)
            })?;

            let persisted_data = TransactionVecReader::from_slice(&buffer)
                .map_err(|err| {
                    let errmsg = format!(
                        "The tx-pool persisted data file [{:?}] is broken, cause: {}",
                        persisted_data_file, err
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

    pub(crate) fn save_into_file(&mut self) -> Result<(), AnyError> {
        let mut persisted_data_file = self.config.persisted_data.clone();
        persisted_data_file.set_extension(format!("v{}", VERSION));

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&persisted_data_file)
            .map_err(|err| {
                let errmsg = format!(
                    "Failed to open the tx-pool persisted data file [{:?}], cause: {}",
                    persisted_data_file, err
                );
                OtherError::new(errmsg)
            })?;

        let txs = TransactionVec::new_builder()
            .extend(self.drain_all_transactions().iter().map(|tx| tx.data()))
            .build();

        file.write_all(txs.as_slice()).map_err(|err| {
            let errmsg = format!(
                "Failed to write the tx-pool persisted data into file [{:?}], cause: {}",
                persisted_data_file, err
            );
            OtherError::new(errmsg)
        })?;
        file.sync_all().map_err(|err| {
            let errmsg = format!(
                "Failed to sync the tx-pool persisted data file [{:?}], cause: {}",
                persisted_data_file, err
            );
            OtherError::new(errmsg)
        })?;
        Ok(())
    }
}
