use crate::TxPool;
use crate::component::pre_pool::{RecoveryMeta, RecoverySnapshotItem};
use ckb_error::{AnyError, OtherError};
use ckb_types::{
    core::TransactionView,
    packed::{TransactionVec, TransactionVecReader},
    prelude::*,
};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

pub(crate) const VERSION: u32 = 2;
const LEGACY_VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"CKBTPV2\0";
const HEADER_BYTES: usize = MAGIC.len() + 8 + 4;
const RECOVERY_META_BYTES: usize = 16 + 4;

#[derive(Clone, Debug, Default)]
pub(crate) struct PersistenceSnapshot {
    pub(crate) accepted: Vec<TransactionView>,
    pub(crate) recovery: Vec<RecoverySnapshotItem>,
}

impl PersistenceSnapshot {
    /// Startup validates every persisted payload again. Accepted ownership
    /// wins a defensive full-hash duplicate; recovery metadata exists to make
    /// a mid-reorg save complete, not to bypass normal admission on restart.
    pub(crate) fn into_transactions(mut self) -> Vec<TransactionView> {
        self.recovery.sort_unstable_by_key(|item| item.meta);
        let mut seen = self
            .accepted
            .iter()
            .map(TransactionView::hash)
            .collect::<HashSet<_>>();
        self.accepted.extend(
            self.recovery
                .into_iter()
                .filter_map(|item| seen.insert(item.tx.hash()).then_some(item.tx)),
        );
        self.accepted
    }
}

fn versioned_path(base: &Path, version: u32) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension(format!("v{version}"));
    path
}

fn broken(path: &Path, detail: impl std::fmt::Display) -> AnyError {
    OtherError::new(format!(
        "The tx-pool persisted data file [{path:?}] is broken, cause: {detail}"
    ))
    .into()
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, AnyError> {
    let mut file = OpenOptions::new().read(true).open(path).map_err(|err| {
        OtherError::new(format!(
            "Failed to open the tx-pool persisted data file [{path:?}], cause: {err}"
        ))
    })?;
    let length = file
        .metadata()
        .map_err(|err| {
            OtherError::new(format!(
                "Failed to stat the tx-pool persisted data file [{path:?}], cause: {err}"
            ))
        })?
        .len();
    if length > max_bytes as u64 {
        return Err(broken(
            path,
            format!("file size {length} exceeds bound {max_bytes}"),
        ));
    }
    let mut buffer = Vec::with_capacity(length as usize);
    file.read_to_end(&mut buffer).map_err(|err| {
        OtherError::new(format!(
            "Failed to read the tx-pool persisted data file [{path:?}], cause: {err}"
        ))
    })?;
    Ok(buffer)
}

fn decode_transactions(path: &Path, bytes: &[u8]) -> Result<Vec<TransactionView>, AnyError> {
    Ok(TransactionVecReader::from_slice(bytes)
        .map_err(|err| broken(path, err))?
        .to_entity()
        .into_iter()
        .map(|tx| tx.into_view())
        .collect())
}

fn decode_v2(path: &Path, bytes: &[u8]) -> Result<PersistenceSnapshot, AnyError> {
    if bytes.len() < HEADER_BYTES || &bytes[..MAGIC.len()] != MAGIC {
        return Err(broken(path, "invalid v2 header"));
    }
    let accepted_len = u64::from_le_bytes(
        bytes[MAGIC.len()..MAGIC.len() + 8]
            .try_into()
            .map_err(|_| broken(path, "missing accepted length"))?,
    );
    let recovery_count = u32::from_le_bytes(
        bytes[MAGIC.len() + 8..HEADER_BYTES]
            .try_into()
            .map_err(|_| broken(path, "missing recovery count"))?,
    ) as usize;
    let metadata_bytes = recovery_count
        .checked_mul(RECOVERY_META_BYTES)
        .ok_or_else(|| broken(path, "recovery metadata length overflow"))?;
    let accepted_len = usize::try_from(accepted_len)
        .map_err(|_| broken(path, "accepted vector length does not fit this platform"))?;
    let accepted_start = HEADER_BYTES
        .checked_add(metadata_bytes)
        .ok_or_else(|| broken(path, "accepted vector offset overflow"))?;
    let recovery_start = accepted_start
        .checked_add(accepted_len)
        .ok_or_else(|| broken(path, "recovery vector offset overflow"))?;
    if recovery_start > bytes.len() {
        return Err(broken(path, "declared sections exceed file length"));
    }

    let mut metadata = Vec::with_capacity(recovery_count);
    for index in 0..recovery_count {
        let start = HEADER_BYTES + index * RECOVERY_META_BYTES;
        let session = u128::from_le_bytes(
            bytes[start..start + 16]
                .try_into()
                .map_err(|_| broken(path, "truncated recovery session"))?,
        );
        let ordinal = u32::from_le_bytes(
            bytes[start + 16..start + RECOVERY_META_BYTES]
                .try_into()
                .map_err(|_| broken(path, "truncated recovery ordinal"))?,
        );
        metadata.push(RecoveryMeta { session, ordinal });
    }
    let accepted = decode_transactions(path, &bytes[accepted_start..recovery_start])?;
    let recovery_txs = decode_transactions(path, &bytes[recovery_start..])?;
    if recovery_txs.len() != metadata.len() {
        return Err(broken(
            path,
            "recovery metadata and transaction counts differ",
        ));
    }
    let recovery = recovery_txs
        .into_iter()
        .zip(metadata)
        .map(|(tx, meta)| RecoverySnapshotItem { tx, meta })
        .collect();
    Ok(PersistenceSnapshot { accepted, recovery })
}

pub(crate) fn write_snapshot(
    base: &Path,
    mut snapshot: PersistenceSnapshot,
) -> Result<(), AnyError> {
    snapshot.recovery.sort_unstable_by_key(|item| item.meta);
    let accepted = TransactionVec::new_builder()
        .extend(snapshot.accepted.iter().map(|tx| tx.data()))
        .build();
    let recovery = TransactionVec::new_builder()
        .extend(snapshot.recovery.iter().map(|item| item.tx.data()))
        .build();
    let recovery_count = u32::try_from(snapshot.recovery.len())
        .map_err(|_| OtherError::new("too many recovery items to persist".to_owned()))?;
    let accepted_len = u64::try_from(accepted.as_slice().len())
        .map_err(|_| OtherError::new("accepted persistence vector is too large".to_owned()))?;

    let path = versioned_path(base, VERSION);
    let tmp = path.with_extension(format!("v{VERSION}.tmp"));
    let write_result = (|| -> Result<(), AnyError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|err| {
                OtherError::new(format!(
                    "Failed to open temp file [{tmp:?}] for tx-pool persistence, cause: {err}"
                ))
            })?;
        file.write_all(MAGIC)?;
        file.write_all(&accepted_len.to_le_bytes())?;
        file.write_all(&recovery_count.to_le_bytes())?;
        for item in &snapshot.recovery {
            file.write_all(&item.meta.session.to_le_bytes())?;
            file.write_all(&item.meta.ordinal.to_le_bytes())?;
        }
        file.write_all(accepted.as_slice())?;
        file.write_all(recovery.as_slice())?;
        file.sync_all().map_err(|err| {
            OtherError::new(format!("Failed to sync temp file [{tmp:?}], cause: {err}"))
        })?;
        drop(file);
        std::fs::rename(&tmp, &path).map_err(|err| {
            OtherError::new(format!(
                "Failed to rename temp file [{tmp:?}] to [{path:?}], cause: {err}"
            ))
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

impl TxPool {
    pub(crate) fn load_persistence_snapshot(&self) -> Result<PersistenceSnapshot, AnyError> {
        let v2 = versioned_path(&self.config.persisted_data, VERSION);
        let v1 = versioned_path(&self.config.persisted_data, LEGACY_VERSION);
        let max_bytes = self
            .config
            .max_tx_pool_size
            .saturating_add(self.config.tx_pipeline_resident_size_budget())
            .saturating_mul(2)
            .saturating_add(1024 * 1024);
        let v2_tmp = v2.with_extension(format!("v{VERSION}.tmp"));
        let v1_tmp = v1.with_extension(format!("v{LEGACY_VERSION}.tmp"));
        let _ = std::fs::remove_file(v2_tmp);
        let _ = std::fs::remove_file(v1_tmp);
        if v2.exists() {
            return decode_v2(&v2, &read_bounded(&v2, max_bytes)?);
        }
        if v1.exists() {
            return Ok(PersistenceSnapshot {
                accepted: decode_transactions(&v1, &read_bounded(&v1, max_bytes)?)?,
                recovery: Vec::new(),
            });
        }
        Ok(PersistenceSnapshot::default())
    }

    #[cfg(test)]
    pub(crate) fn load_from_file(&self) -> Result<Vec<TransactionView>, AnyError> {
        self.load_persistence_snapshot()
            .map(PersistenceSnapshot::into_transactions)
    }

    /// Compatibility helper retained for focused persistence unit tests.
    /// Service persistence uses the non-mutating immutable snapshot path.
    #[cfg(test)]
    pub(crate) fn save_into_file(&mut self) -> Result<(), AnyError> {
        let snapshot = PersistenceSnapshot {
            accepted: self.get_all_txs(),
            recovery: Vec::new(),
        };
        write_snapshot(&self.config.persisted_data, snapshot)?;
        let chain = self.cloned_snapshot();
        self.clear(chain);
        Ok(())
    }
}
