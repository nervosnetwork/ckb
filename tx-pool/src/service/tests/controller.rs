use super::*;
use crate::authority::service::VerificationControl;
use crate::service::{
    AsyncRequest, ChainControl, DEFAULT_CHANNEL_SIZE, Notify, NotifyTxBatch, RemoteTxBatchOutcome,
    RemoteTxSubmission,
};
use crate::test_support::genesis_snapshot;
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use ckb_script::ChunkCommand;
use ckb_types::{core::BlockBuilder, prelude::Entity};
use futures_util::FutureExt;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn controller(sender: mpsc::Sender<Message>) -> TxPoolController {
    let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);

    TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    }
}

fn full_controller() -> (TxPoolController, mpsc::Receiver<Message>) {
    let (sender, receiver) = mpsc::channel(1);
    assert!(
        sender
            .try_send(Message::NotifyTxs(Notify::new(
                NotifyTxBatch::try_new(Vec::new()).expect("empty relay batch is valid"),
            )))
            .is_ok(),
        "fixture fills the bounded controller channel"
    );
    (controller(sender), receiver)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_compute_and_template_waits_preserve_reserved_read_routes() {
    use ckb_types::core::{
        TransactionBuilder,
        tx_pool::{Reject, TxStatus},
    };

    let (sender, mut ordinary) = mpsc::channel(2);
    let (query_sender, mut queries) = mpsc::channel(2);
    let mut controller = controller(sender);
    controller.query_sender = query_sender;
    let mut callers = Vec::new();
    let mut held = Vec::new();
    for _ in 0..2 {
        let client = controller.clone();
        callers.push(tokio::task::spawn_blocking(move || {
            client.test_accept_tx(TransactionBuilder::default().build())
        }));
        let message = tokio::time::timeout(Duration::from_secs(5), ordinary.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::TestAcceptTx(request) = message else {
            panic!("ordinary verification must use the compute handler channel");
        };
        held.push(request);
    }
    let client = controller.clone();
    let template = tokio::task::spawn_blocking(move || client.get_block_template(None, None, None));
    let Message::BlockTemplate(template_request) =
        tokio::time::timeout(Duration::from_secs(5), ordinary.recv())
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("template refresh cannot occupy the reserved read handlers");
    };
    // Keep both ordinary responses pending. Receiving the next requests on the
    // reserved channel establishes independence without relying on VM duration.
    let client = controller.clone();
    let status = tokio::task::spawn_blocking(move || client.get_tx_status(Byte32::default()));
    let Message::GetTxStatus(request) =
        tokio::time::timeout(Duration::from_secs(5), queries.recv())
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("status query keeps reserved service capacity");
    };
    request
        .responder
        .send(Ok((TxStatus::Unknown, None)))
        .unwrap();
    assert_eq!(
        status.await.unwrap().unwrap().unwrap(),
        (TxStatus::Unknown, None)
    );

    #[cfg(feature = "internal")]
    {
        let client = controller.clone();
        let packaging = tokio::task::spawn_blocking(move || {
            crate::callback::with_callback_context(|| client.package_txs(None))
        });
        let Message::PackageTxs(request) =
            tokio::time::timeout(Duration::from_secs(5), queries.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("callback packaging keeps the reserved read route");
        };
        request.responder.send(Vec::new()).unwrap();
        assert!(packaging.await.unwrap().unwrap().is_empty());
    }

    let callback = tokio::task::spawn_blocking(move || {
        crate::callback::with_callback_context(|| {
            controller.test_accept_tx(TransactionBuilder::default().build())
        })
    });
    let Message::TestAcceptTx(request) =
        tokio::time::timeout(Duration::from_secs(5), queries.recv())
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("callback verification cannot wait behind its publication dependents");
    };
    request
        .responder
        .send(Err(Reject::Full("fixture".into())))
        .unwrap();
    assert!(callback.await.unwrap().unwrap().is_err());
    template_request
        .responder
        .send(Err(ckb_error::OtherError::new("fixture".to_owned()).into()))
        .unwrap();
    assert!(template.await.unwrap().unwrap().is_err());
    for request in held {
        request
            .responder
            .send(Err(Reject::Full("fixture".into())))
            .unwrap();
    }
    for caller in callers {
        assert!(caller.await.unwrap().unwrap().is_err());
    }
}

#[test]
fn callback_ibd_mutation_is_refused_before_controller_channel_admission() {
    let (controller, _receiver) = full_controller();
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
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, mut chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(false)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };
    let snapshot = genesis_snapshot();

    assert!(!controller.service_started());
    let publisher_controller = controller.clone();
    let publisher_snapshot = Arc::clone(&snapshot);
    let publisher = tokio::task::spawn_blocking(move || {
        publisher_controller.update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
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
    let ChainReorgArgs::Detailed {
        detached_blocks,
        attached_blocks,
        snapshot: delivered_snapshot,
    } = arguments
    else {
        panic!("an empty bounded reorg remains a detailed transition")
    };
    assert!(detached_blocks.is_empty());
    assert!(attached_blocks.is_empty());
    assert_eq!(delivered_snapshot.tip_hash(), snapshot.tip_hash());
    responder
        .send(())
        .expect("the chain publisher owns the exact Apply completion response");
    publisher
        .await
        .expect("the chain publisher task does not panic")
        .expect("the pre-start transition returns only after Apply completion");
}

#[test]
fn candidate_uncle_is_bounded_and_compacted_before_enqueue() {
    let (sender, mut receiver) = mpsc::channel(1);
    let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    let uncle = BlockBuilder::default().build().as_uncle();
    let retained_bytes =
        BoundedCandidateUncle::payload_limit(u64::try_from(uncle.data().total_size()).unwrap())
            .unwrap();
    assert_eq!(
        retained_bytes,
        uncle.data().total_size() + uncle.hash().as_slice().len()
    );
    assert!(BoundedCandidateUncle::payload_limit(u64::MAX).is_none());
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: retained_bytes - 1,
        signal: CancellationToken::new(),
    };

    let error = controller
        .notify_new_uncle(uncle.clone())
        .expect_err("an over-bound candidate cannot enter the ordinary channel");
    assert!(error.to_string().contains("TooLarge"));
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let controller = TxPoolController {
        candidate_uncle_payload_limit: retained_bytes,
        ..controller
    };
    controller
        .notify_new_uncle(uncle.clone())
        .expect("the exact retained-byte boundary is admitted");
    let Message::NewUncle(notify) = receiver
        .try_recv()
        .expect("the bounded candidate crosses the channel")
    else {
        panic!("candidate notification variant changed")
    };
    assert_eq!(notify.arguments.into_uncle(), uncle);
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

    assert!(!arguments.is_detailed());
    let ChainReorgArgs::ReplaceGeneration {
        snapshot: replacement,
    } = arguments
    else {
        panic!("the zero-byte bound admits only a constant-size replacement")
    };
    assert!(Arc::ptr_eq(&replacement, &snapshot));
}

#[test]
fn closed_reorg_consumer_fails_without_waiting() {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    drop(chain_control_receiver);
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(false)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };

    let error = controller
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            genesis_snapshot(),
        )
        .expect_err("an explicitly disabled tx-pool has no chain consumer");
    assert!(error.to_string().contains("channel closed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_clear_cannot_overtake_a_prior_chain_transition() {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, mut chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };
    let snapshot = genesis_snapshot();

    let reorg_controller = controller.clone();
    let reorg_snapshot = Arc::clone(&snapshot);
    let reorg = tokio::task::spawn_blocking(move || {
        reorg_controller.update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            reorg_snapshot,
        )
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
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, mut chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };

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
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let (verification_command, _verification_commands) =
        VerificationControl::channel(ChunkCommand::Resume);
    drop(chain_control_receiver);
    let administration_gate = AdministrationGate::new();
    let controller = TxPoolController {
        query_sender: sender.clone(),
        sender,
        chain_control_sender,
        verification_command,
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: administration_gate.clone(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    };

    let error = controller
        .clear_pool(genesis_snapshot())
        .expect_err("a closed ordered lane cannot consume the administration");
    assert!(error.to_string().contains("channel closed"));
    let admission = administration_gate
        .try_acquire()
        .expect("failed delivery must release the exact admission capability");
    drop(admission);
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

#[test]
fn asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full() {
    let (controller, _receiver) = full_controller();

    assert_fast_error(
        controller.notify_txs_async(Vec::new()),
        "no available capacity",
    );
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
}

#[tokio::test]
async fn proposal_delivery_preserves_payload_and_closed_network_calls_fail_fast() {
    let (sender, mut receiver) = mpsc::channel(1);
    let client = controller(sender);
    let transaction = ckb_types::core::TransactionBuilder::default()
        .version(7_002u32)
        .build();
    client
        .notify_txs_async(vec![transaction.clone()])
        .await
        .expect("the proposal enters the bounded controller");
    let Some(Message::NotifyTxs(notify)) = receiver.recv().await else {
        panic!("proposal notification missing");
    };
    assert_eq!(
        notify.arguments.into_transactions_for_test(),
        vec![transaction]
    );

    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    let client = controller(sender);
    assert_fast_error(client.notify_txs_async(Vec::new()), "channel closed");
    assert_fast_error(
        client.submit_remote_tx(
            ckb_types::core::TransactionBuilder::default().build(),
            0,
            ckb_network::PeerIndex::from(1),
        ),
        "channel closed",
    );
}

#[test]
fn remote_submit_waits_without_blocking_a_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    runtime.block_on(async {
        let (sender, mut receiver) = mpsc::channel(1);
        let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
        let (verification_command, _verification_commands) =
            VerificationControl::channel(ChunkCommand::Resume);
        let controller = TxPoolController {
            query_sender: sender.clone(),
            sender,
            chain_control_sender,
            verification_command,
            handle: new_background_runtime(),
            started: Arc::new(AtomicBool::new(true)),
            administration_gate: AdministrationGate::new(),
            chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
            candidate_uncle_payload_limit: usize::MAX,
            signal: CancellationToken::new(),
        };
        let transaction = ckb_types::core::TransactionBuilder::default().build();
        let expected = transaction.clone();
        let responder = tokio::spawn(async move {
            let Some(Message::SubmitRemoteTx(AsyncRequest {
                responder,
                arguments,
            })) = receiver.recv().await
            else {
                panic!("remote submission message missing");
            };
            let RemoteTxSubmission { transaction, .. } = arguments;
            assert_eq!(transaction.into_transaction().as_ref(), &expected);
            responder.send(()).expect("test receiver remains present");
        });

        controller
            .submit_remote_tx(transaction, 0, ckb_network::PeerIndex::from(1))
            .await
            .expect("remote submission receives its response");
        responder.await.expect("responder task does not panic");
    });
}

#[test]
fn remote_batch_larger_than_the_controller_capacity_uses_one_queue_slot() {
    const BATCH_LEN: usize = DEFAULT_CHANNEL_SIZE + 1;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    runtime.block_on(async {
        let (sender, mut receiver) = mpsc::channel(1);
        let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
        let (verification_command, _verification_commands) =
            VerificationControl::channel(ChunkCommand::Resume);
        let controller = TxPoolController {
            query_sender: sender.clone(),
            sender,
            chain_control_sender,
            verification_command,
            handle: new_background_runtime(),
            started: Arc::new(AtomicBool::new(true)),
            administration_gate: AdministrationGate::new(),
            chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
            candidate_uncle_payload_limit: usize::MAX,
            signal: CancellationToken::new(),
        };
        let transaction = ckb_types::core::TransactionBuilder::default().build();
        let submissions = (0..BATCH_LEN).map(|_| (transaction.clone(), 0)).collect();
        let responder = tokio::spawn(async move {
            let Some(Message::SubmitRemoteTxBatch(AsyncRequest {
                responder,
                arguments,
            })) = receiver.recv().await
            else {
                panic!("remote batch message missing");
            };
            let (_, submissions) = arguments.into_parts();
            assert_eq!(submissions.len(), BATCH_LEN);
            assert!(
                matches!(receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
                "one network batch consumes exactly one controller queue slot"
            );
            responder
                .send(RemoteTxBatchOutcome::complete(BATCH_LEN))
                .expect("test receiver remains present");
        });

        let outcome = controller
            .submit_remote_txs(submissions, ckb_network::PeerIndex::from(1))
            .expect("the single batch capability is admitted")
            .await
            .expect("the single batch capability receives its response");
        assert_eq!(outcome.offered(), BATCH_LEN);
        assert_eq!(outcome.completed(), BATCH_LEN);
        responder.await.expect("responder task does not panic");
    });
}

#[test]
fn remote_batch_full_controller_fails_before_creating_a_response_owner() {
    let (controller, _receiver) = full_controller();
    let transaction = ckb_types::core::TransactionBuilder::default().build();
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
fn remote_submit_transports_an_unchecked_cycle_declaration() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    runtime.block_on(async {
        let (sender, mut receiver) = mpsc::channel(1);
        let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
        let (verification_command, _verification_commands) =
            VerificationControl::channel(ChunkCommand::Resume);
        let controller = TxPoolController {
            query_sender: sender.clone(),
            sender,
            chain_control_sender,
            verification_command,
            handle: new_background_runtime(),
            started: Arc::new(AtomicBool::new(true)),
            administration_gate: AdministrationGate::new(),
            chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
            candidate_uncle_payload_limit: usize::MAX,
            signal: CancellationToken::new(),
        };
        let transaction = ckb_types::core::TransactionBuilder::default().build();
        let responder = tokio::spawn(async move {
            let Some(Message::SubmitRemoteTx(AsyncRequest {
                responder,
                arguments,
            })) = receiver.recv().await
            else {
                panic!("remote submission message missing")
            };
            assert_eq!(arguments.declared_cycles, u64::MAX);
            responder.send(()).expect("test receiver remains present");
        });

        controller
            .submit_remote_tx(transaction, u64::MAX, ckb_network::PeerIndex::from(2))
            .await
            .expect("the current controller transports raw declared cycles");
        responder.await.expect("responder task does not panic");
    });
}
