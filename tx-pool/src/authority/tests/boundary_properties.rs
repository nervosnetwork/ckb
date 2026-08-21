//! Production replay adapters for the composed boundary reference trace.
//!
//! Each adapter exercises the real owner of one boundary. They share only the
//! symbolic checkpoint vocabulary; no production state transition is copied
//! into this test module.

use super::super::{
    effect::CommittedEffect,
    publisher::{AuthorityEffectEndpoints, EndpointDisposition, compile_committed_effect},
    relay::{AuthorityRelaySink, authority_relay_mailbox},
    runtime::AuthorityRuntime,
    service::AuthorityVerificationControl,
    topology::AuthorityTaskTopology,
};
use super::foundation::{genesis_snapshot, runtime_config};
use crate::{
    callback::Callbacks,
    network::DummyTxPoolNetwork,
    service::{
        AdministrationGate, AsyncRequest, ChainReorgPayloadLimit, Message, Notify,
        RemoteTxSubmission, TxPoolController, TxVerificationResult,
    },
};
use ckb_async_runtime::Handle;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_types::core::TransactionBuilder;
use ckb_verification::cache::init_cache;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundaryTxId(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundaryWitnessId(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundaryPeerId(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundaryRequestId(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundarySource {
    Remote(BoundaryPeerId),
    Proposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundaryKey {
    raw: BoundaryTxId,
    witness: BoundaryWitnessId,
    source: BoundarySource,
    request: BoundaryRequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryControllerState {
    Queued,
    HandlerOwned,
    ResponseSent,
    NotificationFinished,
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryEffectState {
    Committed,
    Claimed,
    Settled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryLifecycleState {
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryEnqueueFailure {
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryRelaySettlement {
    ConservativeReset,
    CircuitDisposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryCheckpoint {
    Controller {
        key: BoundaryKey,
        state: BoundaryControllerState,
    },
    Effect {
        key: BoundaryKey,
        state: BoundaryEffectState,
    },
    Relay {
        key: BoundaryKey,
        settlement: BoundaryRelaySettlement,
    },
    Lifecycle(BoundaryLifecycleState),
}

const SYMBOLIC_TX: BoundaryTxId = BoundaryTxId(1);
const SYMBOLIC_WITNESS: BoundaryWitnessId = BoundaryWitnessId(1);
const SYMBOLIC_PEER: BoundaryPeerId = BoundaryPeerId(7);
const SYMBOLIC_REQUEST: BoundaryRequestId = BoundaryRequestId(1);

fn boundary_key(source: BoundarySource) -> BoundaryKey {
    BoundaryKey {
        raw: SYMBOLIC_TX,
        witness: SYMBOLIC_WITNESS,
        source,
        request: SYMBOLIC_REQUEST,
    }
}

fn expected_controller_success(source: BoundarySource) -> Vec<BoundaryCheckpoint> {
    let key = boundary_key(source);
    let terminal = match source {
        BoundarySource::Remote(_) => BoundaryControllerState::ResponseSent,
        BoundarySource::Proposal => BoundaryControllerState::NotificationFinished,
    };
    vec![
        BoundaryCheckpoint::Controller {
            key,
            state: BoundaryControllerState::Queued,
        },
        BoundaryCheckpoint::Controller {
            key,
            state: BoundaryControllerState::HandlerOwned,
        },
        BoundaryCheckpoint::Controller {
            key,
            state: terminal,
        },
    ]
}

fn expected_failed_enqueue(
    source: BoundarySource,
    failure: BoundaryEnqueueFailure,
) -> Vec<BoundaryCheckpoint> {
    vec![BoundaryCheckpoint::Controller {
        key: boundary_key(source),
        state: match failure {
            BoundaryEnqueueFailure::Full => BoundaryControllerState::Full,
            BoundaryEnqueueFailure::Closed => BoundaryControllerState::Closed,
        },
    }]
}

fn expected_effect_boundary(settlement: BoundaryRelaySettlement) -> Vec<BoundaryCheckpoint> {
    let key = boundary_key(BoundarySource::Remote(SYMBOLIC_PEER));
    vec![
        BoundaryCheckpoint::Effect {
            key,
            state: BoundaryEffectState::Committed,
        },
        BoundaryCheckpoint::Effect {
            key,
            state: BoundaryEffectState::Claimed,
        },
        BoundaryCheckpoint::Relay { key, settlement },
        BoundaryCheckpoint::Effect {
            key,
            state: BoundaryEffectState::Settled,
        },
    ]
}

fn controller(sender: mpsc::Sender<Message>) -> TxPoolController {
    let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
    let (_verification_control, verification_command) =
        AuthorityVerificationControl::channel(ChunkCommand::Resume);
    TxPoolController {
        sender,
        chain_control_sender,
        verification_command,
        handle: Handle::new(tokio::runtime::Handle::current(), None),
        started: Arc::new(AtomicBool::new(true)),
        administration_gate: AdministrationGate::new(),
        chain_reorg_payload_limit: ChainReorgPayloadLimit::for_test(usize::MAX),
        candidate_uncle_payload_limit: usize::MAX,
        signal: CancellationToken::new(),
    }
}

async fn wait_for_queued_message(receiver: &mpsc::Receiver<Message>) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while receiver.len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the controller fixture reaches its bounded queued cut");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_controller_remote_handoff_refines_queued_handler_and_response_cuts() {
    let source = BoundarySource::Remote(SYMBOLIC_PEER);
    let key = boundary_key(source);
    let expected = expected_controller_success(source);
    let (sender, mut receiver) = mpsc::channel(1);
    let controller = controller(sender);
    let transaction = TransactionBuilder::default().version(7_001u32).build();
    let expected_transaction = transaction.clone();
    let submitter = tokio::spawn(async move {
        controller
            .submit_remote_tx(
                transaction,
                0,
                PeerIndex::from(usize::from(SYMBOLIC_PEER.0)),
            )
            .await
    });

    wait_for_queued_message(&receiver).await;
    let mut actual = vec![BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::Queued,
    }];
    let Some(Message::SubmitRemoteTx(AsyncRequest {
        responder,
        arguments,
    })) = receiver.recv().await
    else {
        panic!("the remote controller fixture must retain the exact request variant");
    };
    let RemoteTxSubmission { transaction, .. } = arguments;
    assert_eq!(
        transaction.into_transaction().as_ref(),
        &expected_transaction
    );
    actual.push(BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::HandlerOwned,
    });
    responder
        .send(())
        .expect("the remote submitter still owns its response receiver");
    submitter
        .await
        .expect("the remote controller task remains healthy")
        .expect("the handler response completes the remote handoff");
    actual.push(BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::ResponseSent,
    });

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_controller_proposal_handoff_refines_notification_cuts() {
    let source = BoundarySource::Proposal;
    let key = boundary_key(source);
    let expected = expected_controller_success(source);
    let (sender, mut receiver) = mpsc::channel(1);
    let controller = controller(sender);
    let transaction = TransactionBuilder::default().version(7_002u32).build();
    let expected_transaction = transaction.clone();

    controller
        .notify_txs_async(vec![transaction])
        .await
        .expect("the proposal notification enters the bounded controller");
    let mut actual = vec![BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::Queued,
    }];
    let Some(Message::NotifyTxs(Notify { arguments })) = receiver.recv().await else {
        panic!("the proposal controller fixture must retain the exact notification variant");
    };
    assert_eq!(
        arguments.into_transactions_for_test(),
        vec![expected_transaction]
    );
    actual.push(BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::HandlerOwned,
    });
    actual.push(BoundaryCheckpoint::Controller {
        key,
        state: BoundaryControllerState::NotificationFinished,
    });

    assert_eq!(actual, expected);
}

async fn controller_failure(
    source: BoundarySource,
    failure: BoundaryEnqueueFailure,
) -> Vec<BoundaryCheckpoint> {
    let (sender, receiver) = mpsc::channel(1);
    let controller = controller(sender);
    let mut retained_receiver = Some(receiver);
    match failure {
        BoundaryEnqueueFailure::Full => {
            controller
                .notify_txs_async(Vec::new())
                .await
                .expect("the first notification fills the capacity-one fixture");
        }
        BoundaryEnqueueFailure::Closed => {
            drop(retained_receiver.take());
        }
    }
    let transaction = TransactionBuilder::default().version(7_003u32).build();
    let error = match source {
        BoundarySource::Remote(peer) => controller
            .submit_remote_tx(transaction, 0, PeerIndex::from(usize::from(peer.0)))
            .await
            .expect_err("the failed remote handoff has no controller owner"),
        BoundarySource::Proposal => controller
            .notify_txs_async(vec![transaction])
            .await
            .expect_err("the failed proposal handoff has no controller owner"),
    };
    let error = error.to_string();
    match failure {
        BoundaryEnqueueFailure::Full => assert!(error.contains("no available capacity")),
        BoundaryEnqueueFailure::Closed => assert!(error.contains("channel closed")),
    }
    drop(retained_receiver);
    vec![BoundaryCheckpoint::Controller {
        key: boundary_key(source),
        state: match failure {
            BoundaryEnqueueFailure::Full => BoundaryControllerState::Full,
            BoundaryEnqueueFailure::Closed => BoundaryControllerState::Closed,
        },
    }]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_controller_full_and_closed_outcomes_refine_both_relay_sources() {
    for source in [
        BoundarySource::Remote(SYMBOLIC_PEER),
        BoundarySource::Proposal,
    ] {
        for failure in [BoundaryEnqueueFailure::Full, BoundaryEnqueueFailure::Closed] {
            let expected = expected_failed_enqueue(source, failure);
            assert_eq!(controller_failure(source, failure).await, expected);
        }
    }
}

fn runtime() -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the boundary runtime fixture is valid")
}

fn endpoints(relay: AuthorityRelaySink) -> AuthorityEffectEndpoints {
    AuthorityEffectEndpoints::new(
        Arc::new(DummyTxPoolNetwork),
        relay,
        Arc::new(Callbacks::new()),
        None,
    )
}

async fn replay_effect_boundary(settlement: BoundaryRelaySettlement) -> Vec<BoundaryCheckpoint> {
    let runtime = runtime();
    runtime
        .queue_generation_reset_for_foundation()
        .expect("the committed reset fits its reserved projection");
    let key = boundary_key(BoundarySource::Remote(SYMBOLIC_PEER));
    let mut actual = vec![BoundaryCheckpoint::Effect {
        key,
        state: BoundaryEffectState::Committed,
    }];
    let receipt = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the committed reset produces one move-only publication receipt");
    actual.push(BoundaryCheckpoint::Effect {
        key,
        state: BoundaryEffectState::Claimed,
    });
    let effect = receipt
        .effects()
        .first()
        .cloned()
        .expect("the reset receipt contains one committed effect");
    assert!(matches!(effect, CommittedEffect::GenerationReset));
    let (relay, relay_receiver) = authority_relay_mailbox(2, 1024 * 1024, 1_024)
        .expect("the boundary relay mailbox is valid");
    let mut relay_receiver = Some(relay_receiver);
    if settlement == BoundaryRelaySettlement::CircuitDisposed {
        drop(relay_receiver.take());
    }
    let mut endpoints = endpoints(relay);
    let disposition = endpoints.publish(compile_committed_effect(effect)).await;
    match settlement {
        BoundaryRelaySettlement::ConservativeReset => {
            assert_eq!(disposition, EndpointDisposition::Published);
            assert!(matches!(
                relay_receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv()),
                Some(TxVerificationResult::GenerationReset)
            ));
        }
        BoundaryRelaySettlement::CircuitDisposed => {
            assert_eq!(disposition, EndpointDisposition::CircuitDisposed);
        }
    }
    actual.push(BoundaryCheckpoint::Relay { key, settlement });
    let completed = receipt.complete_for_foundation();
    let settlement = match settlement {
        BoundaryRelaySettlement::CircuitDisposed => completed.circuit_disposed(),
        BoundaryRelaySettlement::ConservativeReset => completed.published(),
    };
    runtime
        .settle_effect_for_foundation(settlement)
        .expect("the exact publication receipt settles once");
    assert!(
        runtime
            .effect_observation_for_foundation()
            .latest_generation_reset
            .is_none()
    );
    actual.push(BoundaryCheckpoint::Effect {
        key,
        state: BoundaryEffectState::Settled,
    });
    actual
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_effect_relay_boundaries_refine_publication_and_local_circuit_disposal() {
    for settlement in [
        BoundaryRelaySettlement::ConservativeReset,
        BoundaryRelaySettlement::CircuitDisposed,
    ] {
        let expected = expected_effect_boundary(settlement);
        assert_eq!(replay_effect_boundary(settlement).await, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_lifecycle_refines_running_and_stopped_cuts() {
    let runtime = runtime();
    let (relay, _relay_receiver) = authority_relay_mailbox(2, 1024 * 1024, 1_024)
        .expect("the lifecycle relay mailbox is valid");
    let (verification_control, _command_tx) =
        AuthorityVerificationControl::channel(ChunkCommand::Resume);
    let topology = AuthorityTaskTopology::start(
        &Handle::new(tokio::runtime::Handle::current(), None),
        runtime.clone(),
        Arc::new(RwLock::new(init_cache())),
        verification_control,
        endpoints(relay),
        None,
        CancellationToken::new(),
    )
    .expect("the real authority topology starts atomically");
    let mut actual = vec![BoundaryCheckpoint::Lifecycle(
        BoundaryLifecycleState::Running,
    )];
    let report = topology.shutdown(Duration::from_secs(2)).await;
    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
    actual.push(BoundaryCheckpoint::Lifecycle(
        BoundaryLifecycleState::Stopped,
    ));

    let expected = vec![
        BoundaryCheckpoint::Lifecycle(BoundaryLifecycleState::Running),
        BoundaryCheckpoint::Lifecycle(BoundaryLifecycleState::Stopped),
    ];
    assert_eq!(actual, expected);
}
