use super::*;
use crate::authority::service::{Error, Pool, VerificationControl};
use crate::service::{ChainControl, DEFAULT_CHANNEL_SIZE, RemoteTxBatchOutcome, RequestGate};
use crate::test_support::genesis_snapshot;
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_script::ChunkCommand;
use ckb_types::{
    core::{BlockBuilder, TransactionBuilder},
    prelude::Entity,
};
use ckb_verification::cache::init_cache;
use futures_util::FutureExt;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

#[path = "lifecycle.rs"]
mod lifecycle;

/// Controller wired to an open `RequestGate` and an isolated `Pool` so tests
/// exercise real direct execution, backpressure, callback isolation and
/// ordered chain control without fake message receivers.
fn test_controller(
    ordinary_capacity: usize,
    read_capacity: usize,
) -> (
    TxPoolController,
    Arc<Pool>,
    mpsc::Receiver<ChainControl>,
    crate::authority::service::RelaySink,
    crate::authority::service::RelayDrain,
) {
    let handle = new_background_runtime();
    let (pool, sink, drain) = Pool::new(
        TxPoolConfig {
            max_tx_verify_workers: 0,
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        genesis_snapshot(),
        &handle,
        Arc::new(RwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let gate = RequestGate::new(ordinary_capacity, read_capacity);
    gate.open();
    let (chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let controller = TxPoolController {
        pool: Arc::downgrade(&pool),
        gate,
        chain_control_sender,
        verification_command: pool.verification.clone(),
        handle,
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };
    (controller, pool, chain_control_receiver, sink, drain)
}

fn detached_controller(
    ordinary_capacity: usize,
    read_capacity: usize,
    open: bool,
) -> (TxPoolController, mpsc::Receiver<ChainControl>) {
    let gate = RequestGate::new(ordinary_capacity, read_capacity);
    if open {
        gate.open();
    }
    let (chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _bg, _local) = VerificationControl::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        pool: Weak::new(),
        gate,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };
    (controller, chain_control_receiver)
}

fn assert_fast_error<F, T>(future: F, expected: &str)
where
    F: Future<Output = Result<T, AnyError>>,
{
    let error = future
        .now_or_never()
        .expect("controller refusal must be ready without waiting")
        .err()
        .expect("controller admission must fail");
    assert!(error.to_string().contains(expected), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_submission_preserves_error_layers_and_survives_caller_drop() {
    let directory = tempfile::tempdir().unwrap();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (builder, controller, _relay) = crate::service::TxPoolServiceBuilder::new(
        TxPoolConfig {
            max_tx_verify_workers: 0,
            persisted_data: directory.path().join("pool"),
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        genesis_snapshot(),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let generation = builder.start_with_handle(crate::network::DummyTxPoolNetwork);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !controller.service_started() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    // Inner transaction rejection stays distinct from transport errors.
    let tx = TransactionBuilder::default().build();
    let reject = controller
        .submit_local_tx_async(tx.clone())
        .await
        .expect("transport succeeds for a domain rejection")
        .expect_err("empty transaction is rejected by consensus checks");
    assert!(!pool.is_faulted());
    assert!(matches!(
        reject,
        ckb_types::core::tx_pool::Reject::Verification(_)
            | ckb_types::core::tx_pool::Reject::Resolve(_)
    ));

    // Dropping the async future after admission abandons only the reply, not
    // the admitted task: while suspended it still occupies one ordinary slot,
    // and resuming lets it settle cleanly and release its permit.
    controller.suspend_chunk_process().unwrap();
    let mut dropped_request = Box::pin(controller.submit_local_tx_async(tx.clone()));
    assert!(dropped_request.as_mut().now_or_never().is_none());
    assert_eq!(
        controller.gate.ordinary_available(),
        DEFAULT_CHANNEL_SIZE - 1
    );
    drop(dropped_request);
    assert_eq!(
        controller.gate.ordinary_available(),
        DEFAULT_CHANNEL_SIZE - 1,
        "dropping the reply future must not cancel the admitted submission"
    );
    controller.continue_chunk_process().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while controller.gate.ordinary_available() != DEFAULT_CHANNEL_SIZE {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    // Stopped pool returns typed Error::Closed without faulting.
    controller.stop();
    tokio::time::timeout(Duration::from_secs(10), generation)
        .await
        .unwrap()
        .unwrap();
    let closed_err = controller.submit_local_tx_async(tx).await.unwrap_err();
    assert!(closed_err.to_string().contains("channel closed"));
    assert!(!pool.is_faulted());
}

#[tokio::test]
async fn local_submission_refuses_full_closed_and_reentrant_admission() {
    for closed in [false, true] {
        let (controller, _chain) = detached_controller(1, 1, true);
        let _held = if closed {
            controller.gate.close();
            None
        } else {
            Some(controller.gate.try_reserve_ordinary().unwrap())
        };
        let expected = if closed {
            "channel closed"
        } else {
            "no available capacity"
        };
        assert_fast_error(
            controller.submit_local_tx_async(TransactionBuilder::default().build()),
            expected,
        );
        assert!(
            controller
                .submit_local_tx(TransactionBuilder::default().build())
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        let error = crate::callback::with_callback_context(|| {
            controller
                .submit_local_tx_async(TransactionBuilder::default().build())
                .now_or_never()
                .expect("callback rejection precedes the first await")
        })
        .unwrap_err();
        assert!(error.to_string().contains("tx-pool callback"));
        let error = crate::callback::with_callback_context(|| {
            controller.submit_local_tx(TransactionBuilder::default().build())
        })
        .unwrap_err();
        assert!(error.to_string().contains("tx-pool callback"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operational_and_closed_pool_failures_reach_callers_without_faulting_while_fault_escalates()
{
    let (controller, pool, _chain, _sink, _drain) = test_controller(4, 4);
    pool.stop();
    let info_err = tokio::task::spawn_blocking({
        let c = controller.clone();
        move || c.get_tx_pool_info()
    })
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        info_err.downcast_ref::<Error>(),
        Some(Error::Closed)
    ));
    assert!(!pool.is_faulted());

    let remote_err = controller
        .submit_remote_tx(TransactionBuilder::default().build(), 0, 1.into())
        .await
        .unwrap_err();
    assert!(matches!(
        remote_err.downcast_ref::<Error>(),
        Some(Error::Closed)
    ));
    assert!(!pool.is_faulted());

    // Direct settlement of a Fault escalates to pool.fault(), gate.close(), and signal.cancel().
    let (controller, pool, _chain, _sink, _drain) = test_controller(4, 4);
    let err = settle_error::<(), Error>(
        &pool,
        &controller.gate,
        &controller.signal,
        Err(Error::Fault("fixture integrity")),
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<Error>(),
        Some(Error::Fault("fixture integrity"))
    ));
    assert!(pool.is_faulted());
    assert!(controller.gate.is_closed());
    assert!(controller.signal.is_cancelled());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asynchronous_queries_and_callback_reads_use_reserved_lane_when_ordinary_is_full() {
    let (controller, _pool, _chain, _sink, _drain) = test_controller(1, 2);
    let _full_ordinary = controller.gate.try_reserve_ordinary().unwrap();
    assert_eq!(controller.gate.ordinary_available(), 0);

    let tx = TransactionBuilder::default().version(7_003u32).build();
    let proposal = tx.proposal_short_id();
    let hash = tx.hash();

    assert_eq!(
        controller
            .fresh_proposals_filter(vec![proposal.clone()])
            .await
            .unwrap(),
        vec![proposal.clone()]
    );
    assert!(
        controller
            .fetch_txs(HashSet::from([proposal]))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        controller
            .fetch_txs_with_cycles(HashSet::from([hash]))
            .await
            .unwrap()
            .is_empty()
    );

    let client = controller.clone();
    let info = tokio::task::spawn_blocking(move || client.get_tx_pool_info())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.pending_size, 0);

    let client = controller.clone();
    let snapshot = tokio::task::spawn_blocking(move || client.input_snapshot())
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot.inputs.is_empty());

    // Callback preview & packaging use the reserved read lane while ordinary is full.
    let client = controller.clone();
    let preview = tokio::task::spawn_blocking(move || {
        crate::callback::with_callback_context(|| {
            client.test_accept_tx(TransactionBuilder::default().build())
        })
    })
    .await
    .unwrap()
    .unwrap();
    assert!(preview.is_err());

    #[cfg(feature = "internal")]
    {
        let client = controller.clone();
        let packaged = tokio::task::spawn_blocking(move || {
            crate::callback::with_callback_context(|| client.package_txs(None))
        })
        .await
        .unwrap()
        .unwrap();
        assert!(packaged.is_empty());
    }

    // External test_accept_tx uses ordinary capacity and fails fast while full.
    let err = controller
        .test_accept_tx(TransactionBuilder::default().build())
        .unwrap_err();
    assert!(err.to_string().contains("no available capacity"));
}

#[test]
fn asynchronous_queries_refuse_full_or_closed_reserved_lanes_without_using_ordinary_capacity() {
    for closed in [false, true] {
        let (controller, _chain) = detached_controller(2, 1, true);
        let expected = if closed {
            controller.gate.close();
            "channel closed"
        } else {
            let _held = controller.gate.try_reserve_read().unwrap();
            assert_fast_error(
                controller.fresh_proposals_filter(Vec::new()),
                "no available capacity",
            );
            assert_fast_error(
                controller.fetch_txs(HashSet::new()),
                "no available capacity",
            );
            assert_fast_error(
                controller.fetch_txs_with_cycles(HashSet::new()),
                "no available capacity",
            );
            assert_eq!(controller.gate.ordinary_available(), 2);
            continue;
        };
        assert_fast_error(controller.fresh_proposals_filter(Vec::new()), expected);
        assert_fast_error(controller.fetch_txs(HashSet::new()), expected);
        assert_fast_error(controller.fetch_txs_with_cycles(HashSet::new()), expected);
    }
}

#[test]
fn callback_ibd_mutation_is_refused_before_controller_channel_admission() {
    let (controller, _chain) = detached_controller(1, 1, true);
    let _full = controller.gate.try_reserve_ordinary().unwrap();
    let error =
        crate::callback::with_callback_context(|| controller.update_ibd_state(true)).unwrap_err();
    assert!(error.to_string().contains(
        "tx-pool callback cannot synchronously invoke mutating controller operation update_ibd_state"
    ));
    assert!(!crate::callback::in_callback());
}

#[test]
fn reorg_payload_limit_rejects_unrepresentable_combined_residency() {
    let config = TxPoolConfig {
        max_tx_pool_size: usize::MAX,
        ..TxPoolConfig::default()
    };

    assert!(ChainReorgPayloadLimit::from_config(&config).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authoritative_reorg_delivery_is_independent_of_rpc_readiness() {
    let (controller, mut chain_control_receiver) = detached_controller(1, 1, false);
    let snapshot = genesis_snapshot();

    assert!(!controller.service_started());
    let publisher_controller = controller.clone();
    let publisher_snapshot = Arc::clone(&snapshot);
    let publisher = tokio::task::spawn_blocking(move || {
        publisher_controller.update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            publisher_snapshot,
        )
    });

    let delivered = chain_control_receiver
        .recv()
        .await
        .expect("readiness cannot suppress an authoritative chain transition");
    let ChainControl::Reconcile(Request {
        responder,
        arguments,
    }) = delivered
    else {
        panic!("the ordered control must retain the exact chain transition");
    };
    let fork = arguments
        .fork()
        .expect("an empty bounded reorg remains a detailed transition");
    assert!(fork.detached_blocks.is_empty());
    assert!(fork.attached_blocks.is_empty());
    assert!(Arc::ptr_eq(arguments.snapshot(), &snapshot));
    responder
        .send(())
        .expect("the chain publisher owns the exact Apply completion response");
    publisher
        .await
        .expect("the chain publisher task does not panic")
        .expect("the pre-start transition returns only after Apply completion");
}

#[test]
fn candidate_uncle_is_bounded_and_compacted_before_admission() {
    let (mut controller, _pool, _chain, _sink, _drain) = test_controller(2, 2);
    let uncle = BlockBuilder::default().build().as_uncle();
    let retained_bytes =
        BoundedCandidateUncle::payload_limit(u64::try_from(uncle.data().total_size()).unwrap())
            .unwrap();
    assert_eq!(
        retained_bytes,
        uncle.data().total_size() + uncle.hash().as_slice().len()
    );
    assert!(BoundedCandidateUncle::payload_limit(u64::MAX).is_none());
    controller.candidate_uncle_payload_limit = retained_bytes - 1;

    let error = controller
        .notify_new_uncle(uncle.clone())
        .expect_err("an over-bound candidate cannot enter admission");
    assert!(error.to_string().contains("TooLarge"));
    assert_eq!(controller.gate.ordinary_available(), 2);

    controller.candidate_uncle_payload_limit = retained_bytes;
    controller
        .notify_new_uncle(uncle)
        .expect("the exact retained-byte boundary is admitted");
    assert_eq!(controller.gate.ordinary_available(), 2);
}

#[test]
fn oversized_reorg_payload_reduces_to_the_exact_snapshot_replacement() {
    let snapshot = genesis_snapshot();
    let arguments = ChainReorgArgs::bounded(
        VecDeque::new(),
        VecDeque::new(),
        Arc::clone(&snapshot),
        ChainReorgPayloadLimit::for_test(0),
    );

    assert!(
        arguments.fork().is_none(),
        "the zero-byte bound admits only a constant-size replacement"
    );
    assert!(Arc::ptr_eq(arguments.snapshot(), &snapshot));
}

#[test]
fn closed_reorg_consumer_fails_without_waiting() {
    let (controller, chain_control_receiver) = detached_controller(1, 1, false);
    drop(chain_control_receiver);

    let error = controller
        .update_tx_pool_for_reorg(VecDeque::new(), VecDeque::new(), genesis_snapshot())
        .expect_err("an explicitly disabled tx-pool has no chain consumer");
    assert!(error.to_string().contains("channel closed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_clear_cannot_overtake_a_prior_chain_transition() {
    let (controller, mut chain_control_receiver) = detached_controller(1, 1, true);
    let snapshot = genesis_snapshot();

    let reorg_controller = controller.clone();
    let reorg_snapshot = Arc::clone(&snapshot);
    let reorg = tokio::task::spawn_blocking(move || {
        reorg_controller.update_tx_pool_for_reorg(VecDeque::new(), VecDeque::new(), reorg_snapshot)
    });

    let Some(ChainControl::Reconcile(Request {
        responder: reorg_responder,
        ..
    })) = chain_control_receiver.recv().await
    else {
        panic!("the prior chain transition must be the first ordered command");
    };

    let clear_controller = controller.clone();
    let clear_snapshot = Arc::clone(&snapshot);
    let clear = tokio::task::spawn_blocking(move || clear_controller.clear_pool(clear_snapshot));

    reorg_responder
        .send(())
        .expect("the ordered driver acknowledges the exact chain Apply");
    reorg
        .await
        .expect("the reorg publisher task does not panic")
        .expect("the reorg publisher observes Apply completion");
    let Some(ChainControl::ClearPool(command)) = chain_control_receiver.recv().await else {
        panic!("clear_pool must follow the already-enqueued chain transition");
    };
    let (admission, Request { responder, .. }) = command.into_parts();
    drop(admission);
    responder
        .send(())
        .expect("the synchronous clear caller retains its response");
    clear
        .await
        .expect("the clear caller task does not panic")
        .expect("the ordered clear receives its response");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_administration_is_linear_across_controller_clones() {
    let (controller, mut chain_control_receiver) = detached_controller(1, 1, true);

    let first_controller = controller.clone();
    let first =
        tokio::task::spawn_blocking(move || first_controller.clear_pool(genesis_snapshot()));
    let Some(ChainControl::ClearPool(first_command)) = chain_control_receiver.recv().await else {
        panic!("the first public administration must enter the ordered lane");
    };
    let (first_admission, Request { responder, .. }) = first_command.into_parts();

    let concurrent_controller = controller.clone();
    let concurrent = tokio::time::timeout(
        Duration::from_millis(100),
        tokio::task::spawn_blocking(move || concurrent_controller.clear_verify_queue()),
    )
    .await
    .expect("a concurrent public administration must fail without waiting")
    .expect("the concurrent caller task does not panic")
    .expect_err("the unique admission is already held");
    assert!(
        concurrent
            .to_string()
            .contains("another tx-pool administration is already admitted")
    );
    assert!(matches!(
        chain_control_receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    drop(first_admission);
    responder
        .send(())
        .expect("the first synchronous caller retains its response");
    first
        .await
        .expect("the first caller task does not panic")
        .expect("the first public administration receives its response");

    let sequential_controller = controller.clone();
    let sequential =
        tokio::task::spawn_blocking(move || sequential_controller.clear_verify_queue());
    let Some(ChainControl::ClearPipeline(sequential_command)) = chain_control_receiver.recv().await
    else {
        panic!("a sequential public administration must reuse the released admission");
    };
    let (sequential_admission, Request { responder, .. }) = sequential_command.into_parts();
    drop(sequential_admission);
    responder
        .send(())
        .expect("the sequential synchronous caller retains its response");
    sequential
        .await
        .expect("the sequential caller task does not panic")
        .expect("the sequential public administration receives its response");
}

#[test]
fn closed_administration_lane_releases_the_unique_admission() {
    let (controller, chain_control_receiver) = detached_controller(1, 1, true);
    drop(chain_control_receiver);

    let error = controller
        .clear_pool(genesis_snapshot())
        .expect_err("a closed ordered lane cannot consume the administration");
    assert!(error.to_string().contains("channel closed"));
    let admission = controller
        .administration_gate
        .try_acquire()
        .expect("failed delivery must release the exact admission capability");
    drop(admission);
}

#[test]
fn asynchronous_notification_and_remote_calls_fail_fast_when_full_or_closed() {
    let (controller, _chain) = detached_controller(1, 1, true);
    let _full = controller.gate.try_reserve_ordinary().unwrap();
    assert_fast_error(
        controller.notify_txs_async(Vec::new()),
        "no available capacity",
    );
    assert_fast_error(
        controller.submit_remote_tx(
            TransactionBuilder::default().build(),
            0,
            ckb_network::PeerIndex::from(1),
        ),
        "no available capacity",
    );

    let (controller, _chain) = detached_controller(1, 1, true);
    controller.gate.close();
    assert_fast_error(controller.notify_txs_async(Vec::new()), "channel closed");
    assert_fast_error(
        controller.submit_remote_tx(
            TransactionBuilder::default().build(),
            0,
            ckb_network::PeerIndex::from(1),
        ),
        "channel closed",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_batch_larger_than_the_controller_capacity_uses_one_gate_slot() {
    const BATCH_LEN: usize = DEFAULT_CHANNEL_SIZE + 1;
    let (mut controller, pool, _chain, _sink, _drain) = test_controller(1, 1);
    // Keep the gate pre-start so we can observe the single admitted slot
    // deterministically before opening it for the service-owned batch task.
    controller.gate = RequestGate::new(1, 1);
    let submissions: Vec<_> = (0..BATCH_LEN)
        .map(|i| {
            let idx = u32::try_from(i).unwrap();
            (
                TransactionBuilder::default()
                    .input(ckb_types::packed::CellInput::new(
                        OutPoint::new(Byte32::zero(), idx),
                        0,
                    ))
                    .output(ckb_types::packed::CellOutput::default())
                    .output_data(ckb_types::packed::Bytes::default())
                    .build(),
                0,
            )
        })
        .collect();
    let expected: std::collections::BTreeSet<_> =
        submissions.iter().map(|(tx, _)| tx.hash()).collect();

    // Even with gate ordinary capacity = 1, the entire 513-tx batch consumes
    // exactly one admission slot. Dropping the response future does NOT cancel
    // the admitted service task; opening the gate lets it complete and release
    // the slot.
    let future = controller
        .submit_remote_txs(submissions, ckb_network::PeerIndex::from(1))
        .expect("the single batch capability is admitted");
    assert_eq!(controller.gate.ordinary_available(), 0);
    drop(future);
    assert_eq!(
        controller.gate.ordinary_available(),
        0,
        "dropping the response future must not cancel the admitted remote batch"
    );
    controller.gate.open();
    tokio::time::timeout(Duration::from_secs(10), async {
        while controller.gate.ordinary_available() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(pool.owner_hashes(), expected);
}

#[test]
fn empty_remote_batch_completes_even_when_the_controller_is_full_or_closed() {
    let (controller, _chain) = detached_controller(1, 1, true);
    let _full = controller.gate.try_reserve_ordinary().unwrap();
    let assert_empty_completes = || {
        let outcome = controller
            .submit_remote_txs(Vec::new(), ckb_network::PeerIndex::from(1))
            .expect("an empty batch needs no gate capacity")
            .now_or_never()
            .expect("an empty batch needs no await")
            .expect("an empty batch completes successfully");
        assert_eq!(outcome.offered(), 0);
        assert_eq!(outcome.completed(), 0);
    };
    assert_empty_completes();
    assert_eq!(controller.gate.ordinary_available(), 0);
    controller.gate.close();
    assert_empty_completes();
}

#[test]
fn remote_batch_full_controller_fails_before_creating_a_response_owner() {
    let (controller, _chain) = detached_controller(1, 1, true);
    let _full = controller.gate.try_reserve_ordinary().unwrap();
    let transaction = TransactionBuilder::default().build();
    let result =
        controller.submit_remote_txs(vec![(transaction, 0)], ckb_network::PeerIndex::from(1));
    let error = result
        .err()
        .expect("a full controller rejects the bounded batch synchronously");
    assert!(
        error.to_string().contains("no available capacity"),
        "{error}"
    );
}

#[test]
fn remote_batch_result_reports_only_its_exact_committed_prefix() {
    let outcome = RemoteTxBatchOutcome::failed(3, 2, Error::Closed.into());
    let (offered, completed, error) = outcome.into_parts();
    assert_eq!((offered, completed), (3, 2));
    assert!(matches!(
        error.unwrap().downcast_ref::<Error>(),
        Some(Error::Closed)
    ));
    let outcome = RemoteTxBatchOutcome::complete(3);
    assert_eq!((outcome.offered(), outcome.completed()), (3, 3));
}

#[test]
fn local_competing_progress_keeps_the_public_typed_error_discriminator() {
    let (controller, pool, _chain, _sink, _drain) = test_controller(1, 1);
    let err = settle_error::<(), crate::service::LocalRemovalCompetingProgress>(
        &pool,
        &controller.gate,
        &controller.signal,
        Err(crate::service::LocalRemovalCompetingProgress),
    )
    .unwrap_err();
    assert!(
        err.downcast_ref::<crate::service::LocalRemovalCompetingProgress>()
            .is_some()
    );
    assert!(!pool.is_faulted());
}
