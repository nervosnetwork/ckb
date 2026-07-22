//! Regression test: the persisted pool file must be topologically ordered.
//!
//! `get_all_txs` iterates the slab-backed entry map in slot order (vacant
//! slots are reused), which is not the insertion order. The reload path
//! replays the file serially without retry, so a child stored before its
//! parent would be dropped as stale (and recorded in recent_reject).
//! `TxPool::save_into_file` therefore sorts parents before children before
//! writing.

use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use ckb_types::core::{Capacity, TransactionBuilder, TransactionView};
use ckb_types::packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint};
use ckb_types::prelude::*;
use std::collections::HashMap;

use super::harness::{WorkerSet, harness};
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx};

#[tokio::test]
async fn persisted_file_is_topologically_ordered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let service = harness(1).workers(WorkerSet::None).build().service;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.config.persisted_data = tmp.path().join("tx_pool");

    // Several chains with interleaved insertions, plus removals to free
    // early slab slots so later insertions reuse them out of order.
    let insert_chain =
        |tx_pool: &mut crate::TxPool, seed: u8, len: usize| -> Vec<TransactionView> {
            let mut prev = Byte32::new([seed; 32]);
            let mut chain = Vec::new();
            for _ in 0..len {
                let link = build_tx(vec![(&prev, 0)], 1);
                prev = link.hash();
                tx_pool
                    .pool_map
                    .add_entry(
                        TxEntry::dummy_resolve(
                            link.clone(),
                            MOCK_CYCLES,
                            Capacity::shannons(1_000),
                            MOCK_SIZE,
                        ),
                        Status::Pending,
                    )
                    .unwrap();
                chain.push(link);
            }
            chain
        };

    let mut chains = Vec::new();
    for seed in 1..=3u8 {
        chains.push(insert_chain(&mut tx_pool, seed, 6));
    }
    for chain in &chains {
        tx_pool.pool_map.remove_entry(&chain[1].proposal_short_id());
    }
    chains.push(insert_chain(&mut tx_pool, 250, 4));

    tx_pool.save_into_file().expect("save succeeds");
    let loaded = tx_pool.load_from_file().expect("load succeeds");
    assert!(!loaded.is_empty());

    // For every parent/child pair present in the file, the parent must
    // come first.
    let position: HashMap<Byte32, usize> = loaded
        .iter()
        .enumerate()
        .map(|(i, tx)| (tx.hash(), i))
        .collect();
    for (child_pos, tx) in loaded.iter().enumerate() {
        for input in tx.input_pts_iter() {
            if let Some(&parent_pos) = position.get(&input.tx_hash()) {
                assert!(
                    parent_pos < child_pos,
                    "parent must precede child in the persisted file"
                );
            }
        }
    }
}

/// Build a transaction that passes full contextual verification on the
/// harness chain: it spends a real cell secured by the always-success
/// script and carries the always-success lock on its own output, so its
/// children can spend it too.
fn build_resolvable_tx(input: &OutPoint, output_capacity: usize) -> TransactionView {
    let (_, _, always_success_script) = ckb_test_chain_utils::always_success_cell();
    TransactionBuilder::default()
        .cell_dep(
            CellDep::new_builder()
                .out_point(ckb_test_chain_utils::create_always_success_out_point())
                .build(),
        )
        .input(CellInput::new(input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(output_capacity).unwrap())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(ckb_types::bytes::Bytes::default().pack())
        .build()
}

/// Replay must be robust against a child-first persisted vector: the load
/// path (`load_persisted_data`) sorts parents before children, so a
/// dependent chain restores completely even when the file order is wrong.
/// Without the sort, the child would hit missing-input and be dropped (and
/// recorded) before its parent ever shows up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_first_persisted_vector_is_replayed_in_dependency_order() {
    use crate::tx_source::TxSource;

    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let funding = h.out_points[0].clone();

    let parent = build_resolvable_tx(&funding, 4_000);
    let child = build_resolvable_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    // Control: without ordering, the child cannot be processed at all —
    // this is the failure mode the load-side sort exists to prevent.
    let ret = service.process_tx(child.clone(), TxSource::local()).await;
    assert!(
        matches!(ret, Err(crate::error::Reject::Resolve(_))),
        "child-first replay without sorting must fail with a resolve error, got {ret:?}"
    );
    assert!(
        service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .get_by_id(&child_id)
            .is_none(),
        "the failed child must not linger anywhere"
    );

    // The load path: sort, then replay serially (exactly what
    // `load_persisted_data` does before `submit_local_tx`).
    let mut persisted = vec![child.clone(), parent.clone()];
    crate::service::TxPoolService::sort_txs_by_dependencies(&mut persisted);
    for tx in persisted {
        service
            .process_tx(tx, TxSource::local())
            .await
            .expect("replay must succeed once sorted");
    }

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.contains_proposal_id(&parent_id),
        "parent must be restored"
    );
    assert!(
        pool.contains_proposal_id(&child_id),
        "child must be restored after its parent"
    );
}

/// A dependent chain already in the pool must survive a save + restart
/// round trip with every link restored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependent_chain_survives_save_and_restart() {
    use crate::tx_source::TxSource;

    let tmp = tempfile::TempDir::new().unwrap();
    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let funding = h.out_points[0].clone();

    let parent = build_resolvable_tx(&funding, 4_000);
    let child = build_resolvable_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    service
        .process_tx(parent.clone(), TxSource::local())
        .await
        .unwrap();
    service
        .process_tx(child.clone(), TxSource::local())
        .await
        .unwrap();

    // Persist (which clears the in-memory pool) and reload, mimicking a
    // restart.
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool.config.persisted_data = tmp.path().join("tx_pool");
        tx_pool.save_into_file().expect("save succeeds");
        assert!(
            tx_pool.pool_map.get_by_id(&parent_id).is_none(),
            "save must leave the in-memory pool drained"
        );
    }
    let mut loaded = {
        let tx_pool = service.pool.tx_pool.read().await;
        tx_pool.load_from_file().expect("load succeeds")
    };

    // Replay through the same path `load_persisted_data` uses.
    crate::service::TxPoolService::sort_txs_by_dependencies(&mut loaded);
    for tx in loaded {
        service
            .process_tx(tx, TxSource::local())
            .await
            .expect("reload must succeed");
    }

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.contains_proposal_id(&parent_id),
        "parent must survive save + restart"
    );
    assert!(
        pool.contains_proposal_id(&child_id),
        "child must survive save + restart"
    );
}

/// Dropping every controller sender is a graceful shutdown trigger too. The
/// dispatcher must cancel and join the pipeline before saving, then expose a
/// completed handle only after the accepted pool is durable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatcher_channel_close_quiesces_workers_and_persists_pool() {
    use ckb_async_runtime::Handle;
    use ckb_fee_estimator::FeeEstimator;
    use ckb_stop_handler::CancellationToken;
    use ckb_verification::cache::init_cache;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;

    let tmp = tempfile::TempDir::new().unwrap();
    let (consensus, funding) = super::pipeline::test_consensus(1);
    let consensus = Arc::new(consensus);
    let (_store, snapshot) = super::pipeline::snapshot_with_genesis(consensus);
    let mut config = super::pipeline::tx_pool_config();
    config.persisted_data = tmp.path().join("tx_pool");

    let runtime = Handle::new(tokio::runtime::Handle::current(), None);
    let (relay_sender, relay_receiver) = ckb_channel::bounded(16);
    let relay_drain = tokio::task::spawn_blocking(move || while relay_receiver.recv().is_ok() {});
    let (mut builder, controller) = crate::TxPoolServiceBuilder::new(
        config.clone(),
        Arc::clone(&snapshot),
        None,
        Arc::new(RwLock::new(init_cache())),
        &runtime,
        relay_sender,
        FeeEstimator::new_dummy(),
    );
    // Keep this test independent from the process-wide stop token. The only
    // shutdown trigger below is closing every controller sender.
    builder.signal_receiver = CancellationToken::new();
    let dispatcher = builder.start_with_handle(super::chunk::dummy_network());

    let tx = build_resolvable_tx(&funding[0], 4_000);
    let tx_id = tx.proposal_short_id();
    controller
        .submit_local_tx(tx)
        .expect("dispatcher responds")
        .expect("transaction commits before shutdown");
    assert_eq!(
        controller.sender.strong_count(),
        1,
        "the user-facing controller must be the last message sender after startup"
    );

    let weak_sender = controller.sender.downgrade();
    drop(controller);
    assert!(
        weak_sender.upgrade().is_none(),
        "dropping the final controller must close the message channel"
    );
    tokio::time::timeout(Duration::from_secs(10), dispatcher)
        .await
        .expect("channel-close shutdown must finish")
        .expect("dispatcher task must not panic");
    tokio::time::timeout(Duration::from_secs(10), relay_drain)
        .await
        .expect("relay drain must observe service drop")
        .expect("relay drain task must not panic");

    let reloaded = crate::TxPool::new(config, snapshot)
        .load_from_file()
        .expect("channel-close shutdown must write a valid pool file");
    assert!(
        reloaded
            .iter()
            .any(|loaded| loaded.proposal_short_id() == tx_id),
        "accepted transaction must be durable when all senders are dropped"
    );
}
