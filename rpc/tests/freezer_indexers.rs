#[path = "common/freezer.rs"]
mod freezer;

use ckb_async_runtime::new_global_runtime;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, Freezer};
use ckb_test_chain_utils::archive::archive_and_collect;
use ckb_types::prelude::*;
use freezer::{Indexers, attach, block, verified_ext};
use std::time::Duration;

#[test]
fn both_indexers_resume_on_new_generations_and_rollback_to_the_exact_fork() {
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
    // Archive and rotate before attaching. A Secondary that knew only generation zero
    // must not accept a published cursor when it cannot open the new archive prefix.
    {
        let tx = store.begin_transaction();
        tx.insert_block(&a).unwrap();
        tx.insert_block_ext(&a.hash(), &verified_ext()).unwrap();
        tx.commit().unwrap();
    }
    rt.block_on(archive_and_collect(&store, std::slice::from_ref(&a)));
    let commit = ancient.join("COMMIT");
    let saved = ancient.join("TEST-saved-COMMIT");
    std::fs::rename(&commit, &saved).unwrap();
    {
        let tx = store.begin_transaction();
        tx.attach_block(&a).unwrap();
        tx.insert_tip_header(&a.header()).unwrap();
        tx.commit().unwrap();
    }
    indexers.notify.notify_new_block(a.clone());
    indexers.hold_at(&genesis);
    std::fs::rename(&saved, &commit).unwrap();
    indexers.notify.notify_new_block(a.clone());
    indexers.wait_for(&[&genesis, &a]);

    // Install a new hot block and omit its extension in the same transaction.
    // All necessary CF handles exist; a header hash check alone cannot catch this.
    {
        let tx = store.begin_transaction();
        tx.insert_block(&b).unwrap();
        tx.attach_block(&b).unwrap();
        tx.insert_block_ext(&b.hash(), &verified_ext()).unwrap();
        tx.insert_tip_header(&b.header()).unwrap();
        tx.delete(ckb_db_schema::COLUMN_BLOCK_EXTENSION, b.hash().as_slice())
            .unwrap();
        tx.commit().unwrap();
    }
    indexers.notify.notify_new_block(b.clone());
    indexers.hold_at(&a);
    {
        let tx = store.begin_transaction();
        assert!(tx.insert_block(&b).is_err());
        tx.insert_raw(
            ckb_db_schema::COLUMN_BLOCK_EXTENSION,
            b.hash().as_slice(),
            b.extension().unwrap().as_slice(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    indexers.wait_for(&[&genesis, &a, &b]);

    // A new canonical block at the same height must replace the old index tip
    // even when no further block is mined. The node restarts, not the indexers.
    drop(store);
    let store = open_store();
    let fork = block(Some(&a), 12);
    attach(
        &store,
        std::slice::from_ref(&fork),
        std::slice::from_ref(&b),
    );
    rt.block_on(archive_and_collect(&store, &[b, fork.clone()]));
    indexers.notify.notify_new_block(fork.clone());
    indexers.wait_for(&[&genesis, &a, &fork]);
    let tip = block(Some(&fork), 13);
    attach(&store, std::slice::from_ref(&tip), &[]);
    indexers.notify.notify_new_block(tip.clone());
    indexers.wait_for(&[&genesis, &a, &fork, &tip]);
    ckb_stop_handler::broadcast_exit_signals();
    runtime.shutdown_timeout(Duration::from_secs(3));
}
