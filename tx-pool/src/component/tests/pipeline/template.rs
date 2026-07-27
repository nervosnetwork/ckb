use super::*;

/// A full rebuild is a derivation from all template inputs, not an incremental
/// copy of the previously published template. Reorg deliberately publishes a
/// reset before detached uncle candidates are retained; the subsequent full
/// rebuild must therefore read the candidate authority itself.
#[tokio::test]
async fn full_rebuild_derives_uncles_from_candidate_authority() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&snapshot),
    )
    .expect("valid test block assembler");
    let candidate = BlockBuilder::default()
        .number(snapshot.tip_number())
        .parent_hash(snapshot.tip_hash())
        .timestamp(snapshot.tip_header().timestamp() + 1)
        .compact_target(snapshot.tip_header().compact_target())
        .epoch(snapshot.tip_header().epoch())
        .build()
        .as_uncle();
    assert!(assembler.candidate_uncles.lock().insert(candidate.clone()));
    assert!(
        assembler.get_current().await.uncles.is_empty(),
        "candidate retention alone must not mutate the published template"
    );

    assert!(
        assembler
            .update_full(&h.service.pool.tx_pool)
            .await
            .unwrap()
    );
    let current = assembler.get_current().await;
    assert_eq!(current.uncles.len(), 1);
    assert_eq!(current.uncles[0].hash, candidate.hash().unpack());
    h.cancel.cancel();
}

/// Reorg phase one journals a blank reset before phase two retains detached
/// candidates. The reset loop is allowed to win that race; phase two must then
/// rebuild from the candidate authority instead of losing the fork payload.
#[tokio::test]
async fn reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention() {
    use crate::block_assembler::{BlockAssembler, ResetApply, ResetNotification};
    use crate::component::tests::harness::{WorkerSet, harness};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service.block_assembler = Some(
        BlockAssembler::new(
            BlockAssemblerConfig {
                code_hash: h256!("0x0"),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "TEST".to_string(),
                update_interval_millis: 60_000,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
                notify_auth_token: None,
            },
            Arc::clone(&snapshot),
        )
        .expect("valid test block assembler"),
    );
    h.service
        .journal_block_assembler_reset(Arc::clone(&snapshot));
    assert_eq!(
        crate::block_assembler::process_reset(
            h.service.clone(),
            ResetNotification::SuppressUntilFull,
        )
        .await,
        ResetApply::Applied,
    );
    let candidate = BlockBuilder::default()
        .number(snapshot.tip_number())
        .parent_hash(snapshot.tip_hash())
        .timestamp(snapshot.tip_header().timestamp() + 1)
        .compact_target(snapshot.tip_header().compact_target())
        .epoch(snapshot.tip_header().epoch())
        .build()
        .as_uncle();

    h.service
        .refresh_block_assembler_after_tx_pool_reorg(vec![candidate.clone()], Arc::clone(&snapshot))
        .await;
    let current = h
        .service
        .block_assembler
        .as_ref()
        .expect("assembler remains installed")
        .get_current()
        .await;
    assert_eq!(current.uncles.len(), 1);
    assert_eq!(current.uncles[0].hash, candidate.hash().unpack());
    h.cancel.cancel();
}

/// Bug #45: a management clear is authoritative state replacement, not a
/// best-effort incremental update. The reset snapshot must survive a saturated
/// wake channel, blank the current template immediately, and notify external
/// miners without waiting for the periodic assembler interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_pool_resets_template_and_notifies_miner_immediately() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::BlockAssemblerMessage;
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use std::sync::atomic::Ordering;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], ISSUE_OUTPUT_CAPACITY as usize - 1);
    h.service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("seed one pending transaction");

    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&snapshot),
    )
    .unwrap();
    assembler
        .update_proposals(&h.service.pool.tx_pool)
        .await
        .unwrap();
    assert_eq!(assembler.get_current().await.proposals.len(), 1);
    let notify_count = Arc::clone(&assembler.notify_count);
    h.service.block_assembler = Some(assembler);

    // Occupy the one-slot wake channel before the clear. Reset authority must
    // live in the journal, not in the channel payload that now cannot enqueue.
    h.service
        .journal_block_assembler_message(BlockAssemblerMessage::Pending);

    h.service.clear_pool(Arc::clone(&snapshot)).await;
    let message = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("clear_pool must not wait for the periodic interval")
        .expect("an existing wake must remain available");
    assert_eq!(message, BlockAssemblerMessage::Pending);

    // The production consumer drains Reset before every received wake.
    crate::block_assembler::process(h.service.clone(), &BlockAssemblerMessage::Reset).await;
    let reconciled = h
        .service
        .relay
        .load_block_assembler_dirty()
        .into_iter()
        .map(|(message, _)| message)
        .collect::<Vec<_>>();
    assert_eq!(
        reconciled,
        vec![
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed,
            BlockAssemblerMessage::Uncle,
        ],
        "an unconditional reset reissues every optimistic partial generation"
    );
    let current = h
        .service
        .block_assembler
        .as_ref()
        .expect("assembler installed")
        .get_current()
        .await;
    assert!(current.proposals.is_empty());
    assert!(current.transactions.is_empty());
    assert_eq!(notify_count.load(Ordering::SeqCst), 1);
    assert_eq!(h.service.pool.tx_pool.read().await.pool_map.size(), 0);
}

/// A template rebuild happens without holding the reset journal lock. If a
/// newer authoritative reset arrives while the older snapshot is being
/// rebuilt, the stale generation must be unable to run its publication closure
/// at all — even when both requests carry the exact same snapshot Arc.
#[tokio::test]
async fn stale_block_assembler_reset_token_cannot_publish() {
    use crate::component::tests::harness::harness;

    let h = harness(1).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();

    h.service
        .relay
        .mark_block_assembler_reset(Arc::clone(&snapshot))
        .unwrap();
    let loaded = h
        .service
        .relay
        .load_block_assembler_reset()
        .expect("older reset is journaled");
    assert!(Arc::ptr_eq(&loaded.snapshot(), &snapshot));
    h.service
        .relay
        .mark_block_assembler_reset(Arc::clone(&snapshot))
        .unwrap();

    let mut stale_publication_ran = false;
    let stale_result = h
        .service
        .relay
        .block_assembler_reset
        .try_apply(&loaded, || {
            stale_publication_ran = true;
        });
    assert!(stale_result.is_none());
    assert!(!stale_publication_ran);

    let current = h
        .service
        .relay
        .load_block_assembler_reset()
        .expect("stale publication must preserve the newer reset");
    assert!(Arc::ptr_eq(&current.snapshot(), &snapshot));

    let applied = h
        .service
        .relay
        .block_assembler_reset
        .try_apply(&current, || "published");
    assert_eq!(applied, Some("published"));
    assert!(h.service.relay.load_block_assembler_reset().is_none());
}

/// Regression for the old load/build/swap/ack split: two consumers could load
/// generation G, a producer could then install G+1, and the slower consumer
/// would still swap G before discovering that its acknowledgement was stale.
/// Exact-token Apply must leave the currently published template untouched.
#[tokio::test]
async fn superseded_reset_cannot_swap_the_current_template() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&snapshot),
    )
    .unwrap();
    h.service
        .relay
        .mark_block_assembler_reset(Arc::clone(&snapshot))
        .unwrap();
    let stale = h
        .service
        .relay
        .load_block_assembler_reset()
        .expect("first reset token is retained");
    let prepared = assembler
        .prepare_reset_template(stale.snapshot())
        .await
        .unwrap();
    h.service
        .relay
        .mark_block_assembler_reset(snapshot)
        .unwrap();

    let before = assembler.current.read().await.clone();
    assert!(
        !assembler
            .publish_reset_template(prepared, &stale, &h.service.relay.block_assembler_reset,)
            .await
            .unwrap()
    );
    let after = assembler.current.read().await.clone();
    assert!(
        Arc::ptr_eq(&before, &after),
        "a superseded reset must not publish even transiently"
    );
    assert!(h.service.relay.block_assembler_reset_pending());
    h.cancel.cancel();
}

/// A partial template update that observes the pool on a newer tip must not
/// consume its dirty generation. Once assembler and pool snapshots converge,
/// the same journal item is retried and conditionally acknowledged.
#[tokio::test]
async fn rejected_duplicate_uncle_does_not_retrigger_template_work() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::BlockAssemblerMessage;
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service.block_assembler = Some(
        BlockAssembler::new(
            BlockAssemblerConfig {
                code_hash: h256!("0x0"),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "TEST".to_string(),
                update_interval_millis: 60_000,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
                notify_auth_token: None,
            },
            Arc::clone(&snapshot),
        )
        .unwrap(),
    );
    let uncle = BlockBuilder::default()
        .parent_hash(snapshot.tip_hash())
        .number(snapshot.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build()
        .as_uncle();

    h.service.receive_candidate_uncle(uncle.clone()).await;
    let dirty = h.service.relay.load_block_assembler_dirty();
    let (_, generation) = dirty
        .iter()
        .find(|(message, _)| *message == BlockAssemblerMessage::Uncle)
        .expect("first candidate marks uncle work");
    h.service
        .relay
        .complete_block_assembler_dirty(&BlockAssemblerMessage::Uncle, *generation);
    assert!(h.service.relay.load_block_assembler_dirty().is_empty());

    h.service.receive_candidate_uncle(uncle).await;
    assert!(
        h.service.relay.load_block_assembler_dirty().is_empty(),
        "a rejected duplicate cannot amplify into repeated template rebuilds"
    );
    h.cancel.cancel();
}

#[tokio::test]
async fn failed_block_assembler_update_retains_dirty_generation_for_retry() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::{BlockAssemblerMessage, TxPoolServiceBuilder};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use ckb_util::LinkedHashSet;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let older = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 60_000,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&older),
    )
    .unwrap();
    h.service.block_assembler = Some(assembler);

    let next_block = BlockBuilder::default()
        .parent_hash(older.tip_hash())
        .number(older.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    let newer = Arc::new(Snapshot::new(
        next_block.header(),
        older.total_difficulty().clone(),
        older.epoch_ext().clone(),
        h.store.store().get_snapshot(),
        Default::default(),
        older.cloned_consensus(),
    ));
    h.service.pool.tx_pool.write().await.snapshot = Arc::clone(&newer);
    h.service
        .journal_block_assembler_message(BlockAssemblerMessage::Pending);

    let mut queue = LinkedHashSet::new();
    assert!(
        !TxPoolServiceBuilder::apply_block_assembler_updates(&h.service, &mut queue).await,
        "tip mismatch must defer rather than acknowledge the proposal update"
    );
    assert_eq!(
        h.service.relay.load_block_assembler_dirty().len(),
        1,
        "failed application retains authoritative dirty work"
    );

    // Restore the matching authoritative snapshot. The next drain must use
    // the retained generation rather than requiring another producer edge.
    h.service.pool.tx_pool.write().await.snapshot = older;
    assert!(TxPoolServiceBuilder::apply_block_assembler_updates(&h.service, &mut queue).await);
    assert!(h.service.relay.load_block_assembler_dirty().is_empty());
}

/// Pool membership and its template delta share one synchronous mutation
/// boundary. In particular, administrative removal must refresh proposals;
/// otherwise an interval-zero assembler can retain a transaction that no
/// longer exists in the pool indefinitely.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_and_removal_journal_block_assembler_delta() {
    use crate::block_assembler::BlockAssembler;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::{BlockAssemblerMessage, RemoveTxOutcome};
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service.block_assembler = Some(
        BlockAssembler::new(
            BlockAssemblerConfig {
                code_hash: h256!("0x0"),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "TEST".to_string(),
                update_interval_millis: 0,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
                notify_auth_token: None,
            },
            snapshot,
        )
        .unwrap(),
    );

    let tx = build_tx(&h.out_points[0], ISSUE_OUTPUT_CAPACITY as usize - 1);
    let tx_hash = tx.hash();
    h.service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("transaction commits");
    let committed = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("commit journals a template wake")
        .expect("assembler channel remains open");
    assert_eq!(committed, BlockAssemblerMessage::Pending);
    crate::block_assembler::process(h.service.clone(), &committed).await;
    assert_eq!(
        h.service
            .block_assembler
            .as_ref()
            .unwrap()
            .get_current()
            .await
            .proposals
            .len(),
        1
    );

    assert_eq!(h.service.remove_tx(tx_hash).await, RemoveTxOutcome::Removed);
    let removed = tokio::time::timeout(Duration::from_secs(1), h.block_assembler_rx.recv())
        .await
        .expect("removal journals a template wake")
        .expect("assembler channel remains open");
    assert_eq!(removed, BlockAssemblerMessage::Pending);
    crate::block_assembler::process(h.service.clone(), &removed).await;
    assert!(
        h.service
            .block_assembler
            .as_ref()
            .unwrap()
            .get_current()
            .await
            .proposals
            .is_empty()
    );

    h.cancel.cancel();
}

/// Topologically sort dependent transactions so parents come before children.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_txs_by_dependencies_orders_parents_before_children() {
    let (_service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    let tx_a = build_tx(issue_out_point, 4_000);
    let tx_b = build_tx(&OutPoint::new(tx_a.hash(), 0), 3_000);
    let tx_c = build_tx(&OutPoint::new(tx_b.hash(), 0), 2_000);

    // Shuffle: child first, then grandchild, then parent.
    let mut txs = vec![tx_c.clone(), tx_b.clone(), tx_a.clone()];
    TxPoolService::sort_txs_by_dependencies(&mut txs).unwrap();

    assert_eq!(txs[0], tx_a);
    assert_eq!(txs[1], tx_b);
    assert_eq!(txs[2], tx_c);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A cycle in the dependency graph should keep the original order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_txs_by_dependencies_keeps_original_order_on_cycle() {
    let (_service, _relay, signal, _store, issue_out_points) = service_with_pipeline(2);
    let input_a = &issue_out_points[0];
    let input_b = &issue_out_points[1];

    let mut txs = vec![input_a.clone(), input_b.clone()]
        .into_iter()
        .map(|out_point| build_tx(&out_point, 4_000))
        .collect::<Vec<_>>();
    let original = txs.clone();
    TxPoolService::sort_txs_by_dependencies(&mut txs).unwrap();
    assert_eq!(txs, original);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
