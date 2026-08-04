use super::*;
use ckb_types::{
    core::{TransactionBuilder, TransactionView},
    packed::{Byte32, CellInput, OutPoint, TransactionVec},
};

fn transaction(seed: u8) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(Byte32::new([seed; 32]), 0), 0))
        .build()
}

fn config(base: &Path) -> TxPoolConfig {
    TxPoolConfig {
        persisted_data: base.to_path_buf(),
        // Keep sparse-file boundary tests small without changing the format's
        // one-megabyte defensive allowance.
        max_tx_pool_size: 1,
        max_tx_pipeline_resident_size: 1,
        ..TxPoolConfig::default()
    }
}

#[tokio::test]
async fn persistence_writer_admits_only_one_snapshot_owner() {
    let writer = Arc::new(PersistenceWriter::default());
    let first = writer.acquire().await;
    let waiting_writer = Arc::clone(&writer);
    let second = tokio::spawn(async move { waiting_writer.acquire().await });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());

    drop(first);
    tokio::time::timeout(std::time::Duration::from_secs(1), second)
        .await
        .expect("released persistence ownership wakes one waiter")
        .expect("persistence waiter does not panic");
}

#[test]
fn persistence_v2_roundtrip_preserves_partitions_and_recovery_order() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let accepted = transaction(1);
    let recovery_first = transaction(3);
    let recovery_second = transaction(2);
    write_snapshot(
        &base,
        PersistenceSnapshot {
            accepted: vec![accepted.clone()],
            recovery: vec![recovery_first.clone(), recovery_second.clone()],
        },
    )
    .expect("v2 snapshot writes atomically");

    let loaded = load_persistence_snapshot(&config(&base)).expect("v2 snapshot is readable");
    assert_eq!(loaded.accepted, vec![accepted]);
    assert_eq!(loaded.recovery, vec![recovery_first, recovery_second]);
}

#[test]
fn persistence_v2_rejects_oversized_file_before_reading_payload() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let config = config(&base);
    let max_bytes = config
        .max_tx_pool_size
        .saturating_add(config.tx_pipeline_resident_size_budget())
        .saturating_mul(2)
        .saturating_add(1024 * 1024);
    let path = versioned_path(&base, VERSION);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .expect("sparse persistence fixture opens");
    file.set_len(
        u64::try_from(max_bytes)
            .expect("test bound fits u64")
            .saturating_add(1),
    )
    .expect("sparse persistence fixture is sized");

    assert!(
        load_persistence_snapshot(&config).is_err(),
        "metadata length is rejected before allocating or reading the sparse payload"
    );
}

#[test]
fn persistence_loader_accepts_legacy_v1_vector() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let tx = transaction(4);
    let vector = TransactionVec::new_builder().push(tx.data()).build();
    std::fs::write(versioned_path(&base, LEGACY_VERSION), vector.as_slice())
        .expect("legacy fixture writes");

    let loaded = load_persistence_snapshot(&config(&base)).expect("legacy vector is accepted");
    assert_eq!(loaded.accepted.len(), 1);
    assert_eq!(loaded.accepted[0].witness_hash(), tx.witness_hash());
    assert!(loaded.recovery.is_empty());
}

#[test]
fn accepted_partition_wins_a_defensive_recovery_duplicate() {
    let accepted = transaction(5);
    let recovery_only = transaction(6);
    let transactions = PersistenceSnapshot {
        accepted: vec![accepted.clone()],
        recovery: vec![accepted.clone(), recovery_only.clone()],
    }
    .into_transactions();

    assert_eq!(transactions, vec![accepted, recovery_only]);
}
