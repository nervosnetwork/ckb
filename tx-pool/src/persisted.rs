use crate::TxPool;
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
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

pub(crate) const VERSION: u32 = 2;
const LEGACY_VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"CKBTPV2\0";
const HEADER_BYTES: usize = 20;
const RECOVERY_META_BYTES: usize = 20;

#[derive(Clone, Debug, Default)]
pub(crate) struct PersistenceSnapshot {
    pub(crate) accepted: Vec<TransactionView>,
    pub(crate) recovery: Vec<TransactionView>,
}

/// Serializes immutable snapshot ownership without an async lock guard.
///
/// Acquisition itself may wait, but the returned lease is moved into the
/// blocking writer before the async caller awaits its join handle. At most one
/// request can therefore copy and retain a full pool snapshot at a time,
/// preserving the original memory/backpressure bound without holding a state
/// lock across `.await`.
#[derive(Default)]
pub(crate) struct PersistenceWriter {
    active: AtomicBool,
    available: Notify,
}

impl PersistenceWriter {
    pub(crate) async fn acquire(self: &Arc<Self>) -> PersistenceLease {
        loop {
            let available = self.available.notified();
            tokio::pin!(available);
            available.as_mut().enable();
            if self
                .active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return PersistenceLease {
                    writer: Arc::clone(self),
                };
            }
            available.await;
        }
    }
}

/// Unique right to materialize and write one persistence snapshot.
pub(crate) struct PersistenceLease {
    writer: Arc<PersistenceWriter>,
}

impl PersistenceLease {
    pub(crate) fn write(self, base: &Path, snapshot: PersistenceSnapshot) -> Result<(), AnyError> {
        write_snapshot(base, snapshot)
    }
}

impl Drop for PersistenceLease {
    fn drop(&mut self) {
        self.writer.active.store(false, Ordering::Release);
        self.writer.available.notify_one();
    }
}

impl PersistenceSnapshot {
    /// Startup validates every persisted payload again. Accepted ownership
    /// wins a defensive full-hash duplicate; recovery metadata exists to make
    /// a mid-reorg save complete, not to bypass normal admission on restart.
    pub(crate) fn into_transactions(mut self) -> Vec<TransactionView> {
        let mut seen = self
            .accepted
            .iter()
            .map(TransactionView::hash)
            .collect::<HashSet<_>>();
        self.accepted.extend(
            self.recovery
                .into_iter()
                .filter_map(|tx| seen.insert(tx.hash()).then_some(tx)),
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
    let max_bytes_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if length > max_bytes_u64 {
        return Err(broken(
            path,
            format!("file size {length} exceeds bound {max_bytes}"),
        ));
    }
    let length = usize::try_from(length)
        .map_err(|_| broken(path, "file length does not fit this platform"))?;
    let mut buffer = Vec::with_capacity(length);
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

fn take_array<const N: usize>(
    path: &Path,
    bytes: &[u8],
    cursor: &mut usize,
    detail: &'static str,
) -> Result<[u8; N], AnyError> {
    let end = cursor
        .checked_add(N)
        .ok_or_else(|| broken(path, "persisted-data cursor overflow"))?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| broken(path, detail))?
        .try_into()
        .map_err(|_| broken(path, detail))?;
    *cursor = end;
    Ok(value)
}

fn decode_v2(path: &Path, bytes: &[u8]) -> Result<PersistenceSnapshot, AnyError> {
    let mut header_cursor = 0;
    if &take_array::<8>(path, bytes, &mut header_cursor, "invalid v2 header")? != MAGIC {
        return Err(broken(path, "invalid v2 header"));
    }
    let accepted_len = u64::from_le_bytes(take_array(
        path,
        bytes,
        &mut header_cursor,
        "missing accepted length",
    )?);
    let recovery_count = u32::from_le_bytes(take_array(
        path,
        bytes,
        &mut header_cursor,
        "missing recovery count",
    )?);
    let recovery_count = usize::try_from(recovery_count)
        .map_err(|_| broken(path, "recovery count does not fit this platform"))?;
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

    let metadata_section = bytes
        .get(HEADER_BYTES..accepted_start)
        .ok_or_else(|| broken(path, "invalid recovery metadata bounds"))?;
    let mut metadata = Vec::with_capacity(recovery_count);
    for chunk in metadata_section.chunks_exact(RECOVERY_META_BYTES) {
        let mut cursor = 0;
        let session = u128::from_le_bytes(take_array(
            path,
            chunk,
            &mut cursor,
            "truncated recovery session",
        )?);
        let ordinal = u32::from_le_bytes(take_array(
            path,
            chunk,
            &mut cursor,
            "truncated recovery ordinal",
        )?);
        metadata.push((session, ordinal));
    }
    let accepted = decode_transactions(
        path,
        bytes
            .get(accepted_start..recovery_start)
            .ok_or_else(|| broken(path, "invalid accepted transaction bounds"))?,
    )?;
    let recovery_txs = decode_transactions(
        path,
        bytes
            .get(recovery_start..)
            .ok_or_else(|| broken(path, "invalid recovery transaction bounds"))?,
    )?;
    if recovery_txs.len() != metadata.len() {
        return Err(broken(
            path,
            "recovery metadata and transaction counts differ",
        ));
    }
    let mut recovery = recovery_txs.into_iter().zip(metadata).collect::<Vec<_>>();
    recovery.sort_unstable_by_key(|(_, meta)| *meta);
    let recovery = recovery.into_iter().map(|(tx, _)| tx).collect();
    Ok(PersistenceSnapshot { accepted, recovery })
}

pub(crate) fn write_snapshot(base: &Path, snapshot: PersistenceSnapshot) -> Result<(), AnyError> {
    let accepted = TransactionVec::new_builder()
        .extend(snapshot.accepted.iter().map(|tx| tx.data()))
        .build();
    let recovery = TransactionVec::new_builder()
        .extend(snapshot.recovery.iter().map(|tx| tx.data()))
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
        for ordinal in 0..recovery_count {
            // v2 retains the retired session slot as zero for on-disk
            // compatibility; ordinal alone defines parent-first recovery.
            file.write_all(&0u128.to_le_bytes())?;
            file.write_all(&ordinal.to_le_bytes())?;
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
}

#[cfg(test)]
#[path = "tests/persisted_seam.rs"]
mod test_seam;
