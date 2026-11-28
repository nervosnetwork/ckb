use crate::shared_builder::open_or_create_db;
use crate::{HeaderIndex, SharedBuilder};
use ckb_app_config::DBConfig;
use ckb_types::{U256, core::HeaderBuilder, core::hardfork::HardForks, packed::Byte32};

#[test]
fn unverified_tip_uses_the_verified_snapshot_without_rewriting_the_candidate() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let before = shared.cloned_snapshot();
    let verified = HeaderBuilder::default()
        .number(10)
        .epoch(before.epoch_ext().number_with_fraction(10))
        .build();
    let total_difficulty = U256::from(1234u64);
    shared.store_snapshot(shared.new_snapshot(
        verified.clone(),
        total_difficulty.clone(),
        before.epoch_ext().clone(),
        before.proposals().clone(),
    ));

    let candidate = HeaderIndex::new(9, Byte32::zero(), U256::zero());
    shared.set_unverified_tip(candidate.clone());
    assert_eq!(
        shared.get_unverified_tip(),
        HeaderIndex::new(10, verified.hash(), total_difficulty)
    );
    // A read must not write an old observation over a concurrent publication.
    assert_eq!(shared.unverified_tip.load().as_ref(), &candidate);
}

#[test]
fn unverified_tip_preserves_equal_and_higher_candidates() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let tip = shared.snapshot().tip_number();
    // An equal-height candidate may belong to another fork. Its identity is
    // still the producer's observation, rather than the canonical tip's hash.
    for number in [tip, tip + 1] {
        let candidate = HeaderIndex::new(number, Byte32::zero(), U256::from(42u64));
        shared.set_unverified_tip(candidate.clone());
        assert_eq!(shared.get_unverified_tip(), candidate);
    }
}

#[test]
fn open_or_create_db_blocks_while_sst_rebuild_holds_exclusive_lock() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let root_dir = tmp_dir.path();
    let db_path = root_dir.join("data").join("db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();

    // Simulate the fallback two-rename gap where active `db` is temporarily absent while
    // `--sst-rebuild` holds the exclusive sibling lock.
    let lock_path = root_dir.join("data").join("db.sst-rebuild.lock");
    let exclusive_lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    exclusive_lock.try_lock().unwrap();

    let config = DBConfig {
        path: db_path.clone(),
        ..Default::default()
    };
    assert!(open_or_create_db("ckb", root_dir, &config, HardForks::new_dev()).is_err());
    assert!(
        !db_path.exists(),
        "startup must not initialize a fresh DB while rebuild holds the sibling lock"
    );

    drop(exclusive_lock);

    // A pending cutover marker must also stop startup without creating an empty `db` directory.
    let marker_path = root_dir.join("data").join("db.sst-rebuild-promoting");
    std::fs::write(&marker_path, "db.migrating\ndb.pre-sst-rebuild-1\n").unwrap();
    assert!(open_or_create_db("ckb", root_dir, &config, HardForks::new_dev()).is_err());
    assert!(
        !db_path.exists(),
        "startup must not create db.path while a cutover recovery marker exists"
    );
    std::fs::remove_file(&marker_path).unwrap();

    let db = open_or_create_db("ckb", root_dir, &config, HardForks::new_dev()).unwrap();
    assert!(db_path.exists());
    assert!(lock_path.exists());
    drop(db);

    // Deferred tmp_dir under db.path must be created cleanly once SharedBuilder opens the DB.
    let nested_tmp = db_path.join("tmp");
    assert!(!nested_tmp.exists());
    let (_handle, _stop_rx, _runtime) = ckb_async_runtime::new_global_runtime(None);
    let (_shared, _pkg) = SharedBuilder::new(
        "ckb",
        root_dir,
        &config,
        None,
        _handle,
        ckb_chain_spec::consensus::Consensus::default(),
    )
    .unwrap()
    .header_map_tmp_dir(Some(nested_tmp.clone()))
    .build()
    .unwrap();
    assert!(nested_tmp.exists());
}
