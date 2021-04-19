use ckb_error::{AnyError, InternalErrorKind, OtherError};
use ckb_types::{core::TransactionView, packed as referenced, prelude::*};
use std::{
    fs::OpenOptions,
    io::{Read as _, Write as _},
    path::PathBuf,
};

mod generated;

pub(crate) use generated::*;

/// The version of the persisted tx-pool data.
pub(crate) const VERSION: u32 = 1;

impl TxPool {
    pub(crate) fn load_from_file(persisted_data_file: &PathBuf) -> Result<Option<Self>, AnyError> {
        let persisted_data_opt = if persisted_data_file.exists() {
            if !persisted_data_file.is_file() {
                let errmsg = format!(
                    "TxPool persisted data [{:?}] exists but it's not a file.",
                    persisted_data_file
                );
                return Err(InternalErrorKind::Config.other(errmsg).into());
            }
            let mut file = OpenOptions::new()
                .read(true)
                .open(&persisted_data_file)
                .map_err(|err| {
                    let errmsg = format!(
                        "Failed to open the tx-pool persisted data [{:?}]: {}",
                        persisted_data_file, err
                    );
                    OtherError::new(errmsg)
                })?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|err| {
                let errmsg = format!(
                    "Failed to read the tx-pool persisted data [{:?}]: {}",
                    persisted_data_file, err
                );
                OtherError::new(errmsg)
            })?;
            TxPoolMetaReader::from_compatible_slice(&buffer)
                .map_err(|err| {
                    let errmsg = format!(
                        "The persisted data of TxPool is broken, please delete it and restart: {}",
                        err
                    );
                    InternalErrorKind::Config.other(errmsg)
                })
                .and_then(|meta| {
                    let version: u32 = meta.version().unpack();
                    if version != VERSION {
                        let errmsg = format!(
                            "The version(={}) of TxPool persisted data is unsupported",
                            version
                        );
                        Err(InternalErrorKind::Config.other(errmsg))
                    } else {
                        Ok(())
                    }
                })?;
            let persisted_data = TxPoolReader::from_slice(&buffer)
                .map_err(|err| {
                    let errmsg = format!(
                        "The persisted data of TxPool is broken, please delete it and restart: {}",
                        err
                    );
                    InternalErrorKind::Config.other(errmsg)
                })?
                .to_entity();
            Some(persisted_data)
        } else {
            None
        };
        Ok(persisted_data_opt)
    }

    pub(crate) fn save_into_file(&self, persisted_data_file: &PathBuf) -> Result<(), AnyError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&persisted_data_file)
            .map_err(|err| {
                let errmsg = format!(
                    "Failed to open the TxPool persisted data file [{:?}]: {}",
                    persisted_data_file, err
                );
                OtherError::new(errmsg)
            })?;
        file.write_all(self.as_slice()).map_err(|err| {
            let errmsg = format!(
                "Failed to write the TxPool persisted data into file [{:?}]: {}",
                persisted_data_file, err
            );
            OtherError::new(errmsg)
        })?;
        file.sync_all().map_err(|err| {
            let errmsg = format!(
                "Failed to sync the TxPool persisted data file [{:?}]: {}",
                persisted_data_file, err
            );
            OtherError::new(errmsg)
        })?;
        Ok(())
    }
}

impl crate::pool::TxPool {
    pub(crate) fn persisted_data(&self) -> TxPool {
        let txs = referenced::TransactionVec::new_builder()
            .extend(self.get_all_transactions().map(TransactionView::data))
            .build();
        TxPool::new_builder()
            .version(VERSION.pack())
            .transactions(txs)
            .build()
    }
}
