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
        // Exercise the internal residency floor for a small serialized pool.
        // Oversized fixtures use sparse files and never allocate their payload.
        max_tx_pool_size: 1,
        ..TxPoolConfig::default()
    }
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
fn persistence_v2_rejects_broken_partition_framing_without_loading_v1() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let legacy = TransactionVec::new_builder()
        .push(transaction(9).data())
        .build();
    std::fs::write(versioned_path(&base, LEGACY_VERSION), legacy.as_slice()).unwrap();
    let empty = TransactionVec::new_builder().build();
    let mut valid = MAGIC.to_vec();
    valid.extend_from_slice(&u64::try_from(empty.as_slice().len()).unwrap().to_le_bytes());
    valid.extend_from_slice(empty.as_slice());
    valid.extend_from_slice(empty.as_slice());

    let mut wrong_magic = valid.clone();
    wrong_magic[0] = b'X';
    let mut accepted_too_long = valid.clone();
    accepted_too_long[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut invalid_accepted = MAGIC.to_vec();
    invalid_accepted.extend_from_slice(&1u64.to_le_bytes());
    invalid_accepted.push(0);
    invalid_accepted.extend_from_slice(empty.as_slice());
    let mut invalid_recovery = valid;
    invalid_recovery.pop();

    for bytes in [
        MAGIC[..4].to_vec(),
        MAGIC.to_vec(),
        wrong_magic,
        accepted_too_long,
        invalid_accepted,
        invalid_recovery,
    ] {
        std::fs::write(versioned_path(&base, VERSION), bytes).unwrap();
        assert!(
            load_persistence_snapshot(&config(&base)).is_err(),
            "a malformed v2 file must not fall back to the valid v1 file"
        );
    }
}

#[test]
fn persistence_v2_rejects_oversized_file_before_reading_payload() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let config = config(&base);
    let max_bytes = persistence_read_bound(&config).expect("fixture read bound is representable");
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
fn persistence_loader_rejects_an_unrepresentable_read_bound_before_io() {
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let mut config = config(&base);
    config.max_tx_pool_size = usize::MAX;

    assert!(
        load_persistence_snapshot(&config).is_err(),
        "configuration overflow cannot relax the persisted-file read bound"
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
fn successful_v2_write_retires_legacy_migration_input() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("pool");
    let legacy = versioned_path(&base, LEGACY_VERSION);
    let old = TransactionVec::new_builder()
        .set(vec![transaction(1).data()])
        .build();
    std::fs::write(&legacy, old.as_slice()).unwrap();
    assert_eq!(
        load_persistence_snapshot(&config(&base))
            .unwrap()
            .accepted
            .len(),
        1
    );
    write_snapshot(&base, PersistenceSnapshot::default()).unwrap();
    assert!(!legacy.exists());
    std::fs::remove_file(versioned_path(&base, VERSION)).unwrap();
    assert!(
        load_persistence_snapshot(&config(&base))
            .unwrap()
            .accepted
            .is_empty()
    );
}

#[test]
fn accepted_partition_wins_a_defensive_recovery_duplicate() {
    let accepted = transaction(5);
    let other_witness = accepted
        .as_advanced_builder()
        .witness(ckb_types::bytes::Bytes::from_static(b"other witness").pack())
        .build();
    assert_eq!(accepted.hash(), other_witness.hash());
    assert_ne!(accepted.witness_hash(), other_witness.witness_hash());
    let recovery_only = transaction(6);
    let transactions = PersistenceSnapshot {
        accepted: vec![accepted.clone()],
        recovery: vec![other_witness, recovery_only.clone()],
    }
    .prepare_replay()
    .expect("replay input is prepared");

    assert_eq!(
        transactions.into_iter().collect::<Vec<_>>(),
        vec![accepted, recovery_only]
    );
}

#[test]
fn persistence_orders_legacy_and_recovery_bodies_without_reordering_accepted() {
    let parent = transaction(7)
        .as_advanced_builder()
        .output(ckb_types::packed::CellOutput::default())
        .output_data(ckb_types::bytes::Bytes::new().pack())
        .build();
    let child = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
        .build();
    let directory = tempfile::TempDir::new().expect("temporary persistence directory");
    let base = directory.path().join("tx_pool");
    let independent = transaction(8);
    for legacy in [
        vec![child.clone(), parent.clone()],
        vec![parent.clone(), child.clone(), independent.clone()],
    ] {
        let vector = TransactionVec::new_builder()
            .extend(legacy.iter().map(TransactionView::data))
            .build();
        std::fs::write(versioned_path(&base, LEGACY_VERSION), vector.as_slice()).unwrap();
        let prepared = load_persistence_snapshot(&config(&base))
            .and_then(PersistenceSnapshot::prepare_replay)
            .unwrap();
        assert_eq!(
            prepared.into_iter().take(2).collect::<Vec<_>>(),
            vec![parent.clone(), child.clone()]
        );
    }
    for snapshot in [
        PersistenceSnapshot {
            accepted: vec![parent.clone(), child.clone(), independent.clone()],
            recovery: Vec::new(),
        },
        PersistenceSnapshot {
            accepted: Vec::new(),
            recovery: vec![child.clone(), parent.clone(), independent],
        },
    ] {
        write_snapshot(&base, snapshot).unwrap();
        let prepared = load_persistence_snapshot(&config(&base))
            .and_then(PersistenceSnapshot::prepare_replay)
            .unwrap();
        assert_eq!(
            prepared.into_iter().take(2).collect::<Vec<_>>(),
            vec![parent.clone(), child.clone()]
        );
    }
}
