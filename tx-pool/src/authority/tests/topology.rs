use super::super::{
    effect::{
        CommittedAcceptance, CommittedEffect, CommittedEntrySnapshot, EffectBatchBound,
        EffectBatchBounds, EffectCapacity, EffectLimits, EffectPolicy,
    },
    plan::{AuthorityFault, PlanError},
    publisher::AuthorityEffectEndpoints,
    relay::{AuthorityRelayReceiver, AuthorityRelaySink, authority_relay_mailbox},
    runtime::AuthorityRuntime,
    service::AuthorityVerificationControl,
    state::{AcceptedStatus, ApplySequence, RawTxHash},
    template_driver::{AuthorityTemplateRole, AuthorityTemplateTask},
    topology::{
        AuthorityGenerationFault, AuthorityShutdownStatus, AuthorityTaskTopology,
        AuthorityTopologyEvent, AuthorityTopologyStartError,
    },
    worker::AuthorityWorkerFaultKind,
};
use super::foundation::{admit_remote, genesis_snapshot, runtime_config, tx};
use crate::{callback::Callbacks, network::DummyTxPoolNetwork, service::TxVerificationResult};
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::{
    core::{Capacity, FeeRate},
    packed::Byte32,
};
use ckb_verification::cache::init_cache;
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, watch};

fn runtime() -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid")
}

fn endpoints(relay: AuthorityRelaySink) -> AuthorityEffectEndpoints {
    AuthorityEffectEndpoints::new(
        Arc::new(DummyTxPoolNetwork),
        relay,
        Arc::new(Callbacks::new()),
        None,
    )
}

fn start(
    runtime: AuthorityRuntime,
    relay: AuthorityRelaySink,
    command: ChunkCommand,
) -> Result<AuthorityTaskTopology, AuthorityTopologyStartError> {
    start_with_endpoints(runtime, endpoints(relay), command)
}

fn start_with_endpoints(
    runtime: AuthorityRuntime,
    endpoints: AuthorityEffectEndpoints,
    command: ChunkCommand,
) -> Result<AuthorityTaskTopology, AuthorityTopologyStartError> {
    let (verification_control, _command_tx) = AuthorityVerificationControl::channel(command);
    start_with_control(runtime, endpoints, verification_control)
}

fn start_with_control(
    runtime: AuthorityRuntime,
    endpoints: AuthorityEffectEndpoints,
    verification_control: AuthorityVerificationControl,
) -> Result<AuthorityTaskTopology, AuthorityTopologyStartError> {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    AuthorityTaskTopology::start(
        &handle,
        runtime,
        Arc::new(RwLock::new(init_cache())),
        verification_control,
        endpoints,
        None,
        CancellationToken::new(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_shutdown_stops_the_paired_verification_generation() {
    let runtime = runtime();
    let (relay, _receiver) = relay_mailbox(2);
    let (verification_control, command_tx) =
        AuthorityVerificationControl::channel(ChunkCommand::Suspend);
    let mut command_observer: watch::Receiver<ChunkCommand> = command_tx.subscribe();
    let topology = start_with_control(runtime, endpoints(relay), verification_control)
        .expect("the complete task topology starts atomically");

    let report = topology.shutdown(Duration::from_secs(2)).await;
    assert!(report.persistence_eligible());
    assert!(command_observer.has_changed().unwrap_or(false));
    assert!(matches!(
        &*command_observer.borrow_and_update(),
        ChunkCommand::Stop
    ));
    command_tx
        .resume()
        .expect("the observer keeps the generation command channel live");
    assert!(matches!(&*command_observer.borrow(), ChunkCommand::Stop));
}

fn relay_mailbox(max_items: usize) -> (AuthorityRelaySink, AuthorityRelayReceiver) {
    authority_relay_mailbox(max_items, 1024 * 1024, 1_024)
        .expect("the topology relay mailbox fixture is valid")
}

fn committed_entry(nonce: u64) -> CommittedEntrySnapshot {
    CommittedEntrySnapshot {
        tx: Arc::new(tx(nonce)),
        cycles: 1,
        size: 2,
        fee: Capacity::shannons(3),
        ancestors_size: 2,
        ancestors_fee: Capacity::shannons(3),
        ancestors_cycles: 1,
        ancestors_count: 1,
        descendants_fee: Capacity::shannons(3),
        descendants_size: 2,
        descendants_cycles: 1,
        descendants_count: 1,
        timestamp: 4,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_clean_shutdown_drains_effects_before_persistence() {
    let runtime = runtime();
    let expected = RawTxHash(Byte32::new([71; 32]));
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::RemoteExpired {
                tx_hash: expected.clone(),
            },
        )
        .expect("the committed effect fits the bounded journal");
    let (relay, relay_rx) = relay_mailbox(4);
    let topology = start(runtime.clone(), relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    let report = topology.shutdown(Duration::from_secs(2)).await;

    assert!(report.persistence_eligible());
    assert!(report.derived_failures().is_empty());
    assert!(runtime.effects_closed_and_drained());
    assert!(matches!(
        relay_rx.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == expected.0
    ));
    assert!(runtime.claim_effect_publisher().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_relay_disconnect_is_local_degradation_not_shutdown() {
    let runtime = runtime();
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::RemoteExpired {
                tx_hash: RawTxHash(Byte32::new([72; 32])),
            },
        )
        .expect("the committed effect fits the bounded journal");
    let (relay, relay_rx) = relay_mailbox(2);
    drop(relay_rx);
    let mut topology = start(runtime.clone(), relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if runtime
                .effect_observation_for_foundation()
                .queued
                .is_empty()
                && runtime
                    .effect_observation_for_foundation()
                    .latest_generation_reset
                    .is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the disconnected relay circuit cannot retain the journal head");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), topology.next_event())
            .await
            .is_err(),
        "an absent external relay consumer must not stop an authority task"
    );

    let report = topology.shutdown(Duration::from_secs(2)).await;
    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_relay_overflow_is_nonblocking_and_reconciled() {
    let runtime = runtime();
    runtime
        .queue_generation_reset_for_foundation()
        .expect("the reserved reconciliation effect commits");
    let (relay, relay_rx) = relay_mailbox(2);
    for byte in [1, 2] {
        assert!(matches!(
            relay.publish(TxVerificationResult::Reject {
                tx_hash: Byte32::new([byte; 32]),
            }),
            super::super::relay::RelayMailboxDisposition::Exact
        ));
    }
    let topology = start(runtime.clone(), relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    let report = topology.shutdown(Duration::from_secs(2)).await;

    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
    assert!(matches!(
        relay_rx.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_relay_overflow_cannot_pin_effect_blocked_compute() {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let mut config = runtime_config();
    config.min_fee_rate = FeeRate::from_u64(1_000);
    let snapshot = genesis_snapshot();
    let effects = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
        ),
    )
    .expect("the narrow fixture admits one effect in each region");
    let runtime = AuthorityRuntime::new_with_effect_limits_for_foundation(
        &config,
        snapshot.consensus(),
        Arc::clone(&snapshot),
        effects,
    )
    .expect("the narrow runtime has one exact effect slot");
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::RemoteExpired {
                tx_hash: RawTxHash(Byte32::new([73; 32])),
            },
        )
        .expect("the first effect occupies the only remote slot");
    let rejected =
        runtime.with_authority_for_foundation(|authority| admit_remote(authority, 1_731, 731));

    let (relay, _relay_rx) = relay_mailbox(2);
    for byte in [1, 2] {
        assert!(matches!(
            relay.publish(TxVerificationResult::Reject {
                tx_hash: Byte32::new([byte; 32]),
            }),
            super::super::relay::RelayMailboxDisposition::Exact
        ));
    }
    let topology = start(runtime.clone(), relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let effects_drained = {
                let observation = runtime.effect_observation_for_foundation();
                observation.queued.is_empty() && observation.latest_generation_reset.is_none()
            };
            let candidate_settled = runtime
                .with_authority_for_foundation(|authority| authority.entry(&rejected).is_none());
            if effects_drained && candidate_settled {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("mailbox overload cannot retain either authority capability");

    let report = topology.shutdown(Duration::from_secs(2)).await;
    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_derived_timeout_does_not_forbid_persistence() {
    let runtime = runtime();
    let (relay, _relay_rx) = relay_mailbox(2);
    let mut topology = start(runtime.clone(), relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");
    topology.install_template_task_for_foundation(AuthorityTemplateTask {
        role: AuthorityTemplateRole::Replacement,
        handle: tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        }),
    });

    let report = topology.shutdown(Duration::from_millis(100)).await;

    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
    assert!(report.derived_failures().iter().any(|failure| matches!(
        failure,
        super::super::topology::AuthorityDerivedTaskFailure::TemplateTimeout(
            super::super::topology::AuthorityTaskRole::Template(_)
        )
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_bounds_one_stuck_callback_without_forbidding_persistence() {
    let runtime = runtime();
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Accepted(CommittedAcceptance::ChainStatusChange {
                entry: committed_entry(1_732),
                status: AcceptedStatus::Pending,
            }),
        )
        .expect("the callback fixture effect fits the bounded journal");
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let release_rx = std::sync::Mutex::new(release_rx);
    let mut callbacks = Callbacks::new();
    callbacks.register_pending(Box::new(move |_| {
        let _ = release_rx
            .lock()
            .expect("the callback release fixture mutex remains healthy")
            .recv();
    }));
    let (relay, _relay_rx) = relay_mailbox(2);
    let endpoints = AuthorityEffectEndpoints::new(
        Arc::new(DummyTxPoolNetwork),
        relay,
        Arc::new(callbacks),
        None,
    );
    let topology = start_with_endpoints(runtime.clone(), endpoints, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    tokio::time::timeout(Duration::from_millis(1_500), async {
        loop {
            let observation = runtime.effect_observation_for_foundation();
            if observation.queued.is_empty() && observation.latest_generation_reset.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the callback timeout disposes the derived endpoint and drains the effect");

    let report = topology.shutdown(Duration::from_millis(100)).await;
    assert!(report.persistence_eligible());
    assert!(runtime.effects_closed_and_drained());
    assert!(report.derived_failures().is_empty());
    release_tx
        .send(())
        .expect("the timed-out callback remains owned by the process runtime");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_claim_conflict_fails_before_any_task_is_spawned() {
    let runtime = runtime();
    let claim = runtime
        .claim_effect_publisher()
        .expect("the fixture holds the sole publisher capability");
    let (first_relay, _first_receiver) = relay_mailbox(2);
    let result = start(runtime.clone(), first_relay, ChunkCommand::Resume);
    assert!(matches!(
        result,
        Err(AuthorityTopologyStartError::EffectPublisherClaimed)
    ));

    drop(claim);
    let (relay, _receiver) = relay_mailbox(2);
    let topology = start(runtime, relay, ChunkCommand::Resume)
        .expect("releasing the construction capability permits one topology");
    assert!(
        topology
            .shutdown(Duration::from_secs(2))
            .await
            .persistence_eligible()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_topology_forbids_persistence_only_after_authority_integrity_loss() {
    let runtime = runtime();
    runtime.with_authority_for_foundation(|authority| {
        let _ = admit_remote(authority, 1_730, 730);
        authority.force_next_sequence(ApplySequence(u128::MAX));
    });
    let (relay, _receiver) = relay_mailbox(2);
    let mut topology = start(runtime, relay, ChunkCommand::Resume)
        .expect("the complete task topology starts atomically");

    let event = tokio::time::timeout(Duration::from_secs(2), topology.next_event())
        .await
        .expect("the exhausted authoritative clock is reported without polling");
    let fault = match event {
        AuthorityTopologyEvent::GenerationInvalid(fault) => fault,
        other => panic!("unexpected topology event: {other:?}"),
    };
    assert!(
        matches!(
            &fault,
            AuthorityGenerationFault::Worker {
                fault: AuthorityWorkerFaultKind::Exchange(failure),
                ..
            } if matches!(
                failure.error(),
                PlanError::Fault(AuthorityFault::CounterExhausted)
            )
        ),
        "unexpected generation fault: {fault:?}"
    );

    let report = topology.invalidate_generation(fault);
    assert!(matches!(
        report.status(),
        AuthorityShutdownStatus::PersistenceForbidden(AuthorityGenerationFault::Worker {
            fault: AuthorityWorkerFaultKind::Exchange(failure),
            ..
        }) if matches!(
            failure.error(),
            PlanError::Fault(AuthorityFault::CounterExhausted)
        )
    ));
}
