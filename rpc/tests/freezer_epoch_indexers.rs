#[path = "common/freezer.rs"]
mod freezer;

use ckb_async_runtime::new_global_runtime;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, Freezer};
use ckb_store::{ChainStore, FreezerController};
use ckb_types::core::BlockView;
use ckb_types::prelude::*;
use freezer::{Indexers, attach, block, verified_ext};
use std::time::Duration;

#[test]
fn both_indexers_follow_epoch_windows_late_forks_and_a_shorter_chain_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().join("db");
    let ancient = root.path().join("ancient");
    let open_store = || {
        let store = ChainDB::new_with_freezer(
            RocksDB::open_in(&primary, COLUMNS),
            Freezer::open_in(&ancient).unwrap(),
            Default::default(),
        );
        store.recover_archive().unwrap();
        store.enable_epoch_segments().unwrap();
        store
    };
    let store = open_store();
    let (rt, _stop, runtime) = new_global_runtime(Some(4));
    let genesis = block(None, 0);
    let a = block(Some(&genesis), 1);
    let b = block(Some(&a), 2);
    attach(&store, std::slice::from_ref(&genesis), &[]);
    let indexers = Indexers::start(root.path(), &primary, &ancient, &rt);
    indexers.wait_for(&[&genesis]);
    // Secondary is already open on segment 0. Open 16 without touching a,
    // attach b, then drive both services across the live sealed/open boundary.
    let store_only = |store: &ChainDB, b: &BlockView| {
        let tx = store.begin_block_transaction(b).unwrap();
        tx.insert_block(b).unwrap();
        tx.insert_block_ext(&b.hash(), &verified_ext()).unwrap();
        tx.commit().unwrap();
    };
    store_only(&store, &a);
    attach(&store, std::slice::from_ref(&a), &[]);
    indexers.notify.notify_new_block(a.clone());
    indexers.wait_for(&[&genesis, &a]);
    store_only(&store, &b);
    assert_eq!(store.db().epoch_segments().windows(), &[0, 16]);
    attach(&store, std::slice::from_ref(&b), &[]);
    indexers.notify.notify_new_block(b.clone());
    indexers.wait_for(&[&genesis, &a, &b]);
    let w = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    let r = rt
        .block_on(store.collect_epoch_segments(&w, || 16, || false, || {}))
        .unwrap();
    assert_eq!(r.dropped_segments, 1);
    rt.block_on(w.shutdown()).unwrap();
    drop(w);
    // A third block cannot be appended from a view whose archive fails validation.
    // Reopen the Primary; keep both indexing services alive. New hash in a
    // retired epoch must be routed to segment 16, even at the same height as a.
    drop(store);
    let store = open_store();
    let late = block(Some(&genesis), 12);
    store_only(&store, &late);
    assert_eq!(
        ckb_db_schema::freezer::routed_segment(
            &store
                .get(ckb_db_schema::COLUMN_BLOCK_ARCHIVE, late.hash().as_slice())
                .unwrap()
        ),
        Some(16)
    );
    let late_b = block(Some(&late), 13);
    let w = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    // Advance the archive cursor while readers are idle, so catch-up must open
    // the newly committed prefix. This does not change the canonical tip.
    let commit = ancient.join("COMMIT");
    let saved = ancient.join("TEST-saved-COMMIT");
    // Displace a+b with a shorter fork. Even without a new child, both services
    // must roll back to the common ancestor and return the exact fork's cells.
    let w_range = rt.block_on(w.append(vec![late.data()])).unwrap();
    std::fs::rename(&commit, &saved).unwrap();
    assert_eq!(store.recover_archive().unwrap(), w_range.end);
    attach(&store, std::slice::from_ref(&late), &[a.clone(), b.clone()]);
    indexers.notify.notify_new_block(late.clone());
    indexers.hold_at(&b);
    std::fs::rename(&saved, &commit).unwrap();
    indexers.notify.notify_new_block(late.clone());
    indexers.wait_for(&[&genesis, &late]);
    attach(&store, std::slice::from_ref(&late_b), &[]);
    indexers.notify.notify_new_block(late_b.clone());
    indexers.wait_for(&[&genesis, &late, &late_b]);
    assert_eq!(store.get_block(&a.hash()).unwrap().data(), a.data());
    assert_eq!(store.get_block(&b.hash()).unwrap().data(), b.data());
    rt.block_on(w.shutdown()).unwrap();
    ckb_stop_handler::broadcast_exit_signals();
    runtime.shutdown_timeout(Duration::from_secs(3));
}
