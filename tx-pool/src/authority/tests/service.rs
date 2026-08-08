use super::super::service::{
    AuthorityDerivedError, AuthorityIntegrityFault, AuthorityProjectionFault, AuthorityRelayDrain,
    AuthorityService, AuthorityServiceAssembly, AuthorityServiceError, AuthorityServiceInputs,
    AuthorityShutdownOutcome, AuthorityVerificationControl, authority_failure_boundary,
    derived_failure_boundary, map_chain_integrity, map_recent_reject_read_error,
    record_candidate_uncle_observation,
};
use super::super::{
    chain_boundary::ChainBoundaryError,
    plan::AuthorityFault,
    rejection::RecentRejectEncodingError,
    runtime::AuthorityRecentRejectReadError,
    template_driver::AuthorityTemplateDriverFault,
    topology::{AuthorityDerivedTaskFailure, AuthorityGenerationFault, AuthorityTaskRole},
    worker::{AuthorityWorkerFaultKind, AuthorityWorkerRole},
};
use super::foundation::{genesis_snapshot, runtime_config};
use crate::{
    PlugTarget, TxEntry, block_assembler::CandidateUncleMutationError, callback::Callbacks,
    component::recent_reject::RecentReject, network::DummyTxPoolNetwork,
    service::TxVerificationResult,
};
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::{
    core::{Capacity, FeeRate, TransactionBuilder},
    packed::{Byte32, CellInput, OutPoint},
};
use ckb_verification::cache::init_cache;
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{RwLock, mpsc};

#[test]
fn uak_operational_failure_classes_follow_ownership_boundaries() {
    let role = AuthorityTaskRole::Worker(AuthorityWorkerRole::Ready);
    assert_eq!(
        authority_failure_boundary(&AuthorityGenerationFault::Worker {
            role,
            fault: AuthorityWorkerFaultKind::Authority(AuthorityFault::EffectProjection),
        }),
        crate::metrics::FailureBoundary::TypedFault
    );
    assert_eq!(
        authority_failure_boundary(&AuthorityGenerationFault::PublisherClosed),
        crate::metrics::FailureBoundary::EffectPublisher
    );
    assert_eq!(
        authority_failure_boundary(&AuthorityGenerationFault::ShutdownTimeout),
        crate::metrics::FailureBoundary::WorkerExit
    );
    assert_eq!(
        derived_failure_boundary(&AuthorityDerivedTaskFailure::TemplateClosed(
            AuthorityTaskRole::Template(
                super::super::template_driver::AuthorityTemplateRole::Transactions,
            ),
        )),
        crate::metrics::FailureBoundary::WorkerExit
    );
}

#[test]
fn uak_only_integrity_faults_invalidate_a_generation() {
    for operational in [
        AuthorityServiceError::Cancelled,
        AuthorityServiceError::BlockAssemblerDisabled,
        AuthorityServiceError::ResourceUnavailable,
        AuthorityServiceError::EffectCapacity,
        AuthorityServiceError::LifecycleClosed,
    ] {
        assert!(AuthorityService::settle_operation_error(operational).is_ok());
    }
    assert!(
        AuthorityService::settle_operation_error(AuthorityServiceError::Integrity(
            AuthorityIntegrityFault::Projection(AuthorityProjectionFault::Membership),
        ))
        .is_err()
    );
}

#[test]
fn uak_ordered_chain_update_has_no_droppable_operational_error() {
    assert_eq!(map_chain_integrity(ChainBoundaryError::Allocation), None);
    assert_eq!(
        map_chain_integrity(ChainBoundaryError::LifecycleClosed),
        Some(AuthorityIntegrityFault::EffectLifecycleClosed)
    );
    assert_eq!(
        map_chain_integrity(ChainBoundaryError::CounterExhausted),
        Some(AuthorityIntegrityFault::CounterExhausted)
    );
    assert_eq!(
        map_chain_integrity(ChainBoundaryError::InvalidFacts),
        Some(AuthorityIntegrityFault::InvalidChainEvidence)
    );
    assert_eq!(
        map_chain_integrity(ChainBoundaryError::InvalidSnapshotEvidence),
        Some(AuthorityIntegrityFault::InvalidChainEvidence)
    );
    assert_eq!(
        map_chain_integrity(ChainBoundaryError::Fault(AuthorityFault::EffectProjection)),
        Some(AuthorityIntegrityFault::Projection(
            AuthorityProjectionFault::Effect
        ))
    );
}

async fn service_assembly() -> (
    AuthorityServiceAssembly,
    Arc<ckb_snapshot::Snapshot>,
    AuthorityRelayDrain,
) {
    service_assembly_with_config(runtime_config()).await
}

async fn service_assembly_with_config(
    config: TxPoolConfig,
) -> (
    AuthorityServiceAssembly,
    Arc<ckb_snapshot::Snapshot>,
    AuthorityRelayDrain,
) {
    service_assembly_with_config_and_recent_reject(config, None).await
}

async fn service_assembly_with_config_and_recent_reject(
    config: TxPoolConfig,
    recent_reject: Option<Arc<RecentReject>>,
) -> (
    AuthorityServiceAssembly,
    Arc<ckb_snapshot::Snapshot>,
    AuthorityRelayDrain,
) {
    let snapshot = genesis_snapshot();
    let (bootstrap, relay) = AuthorityService::prepare(config, Arc::clone(&snapshot))
        .expect("the production relay handoff is constructed before service startup");
    let (verification_control, _command_tx) =
        AuthorityVerificationControl::channel(ChunkCommand::Resume);
    let (_chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let assembly = AuthorityService::assemble(
        &handle,
        AuthorityServiceInputs {
            bootstrap,
            block_assembler: None,
            verification_cache: Arc::new(RwLock::new(init_cache())),
            callbacks: Callbacks::new(),
            network: Arc::new(DummyTxPoolNetwork),
            persistence_writer: Arc::new(crate::persisted::PersistenceWriter::default()),
            recent_reject,
            fee_estimator: FeeEstimator::new_dummy(),
            chain_control_receiver,
            verification_control,
            cancel,
        },
    )
    .await
    .expect("the complete service boundary assembles before ingress opens");
    (assembly, snapshot, relay)
}

#[test]
fn uak_recent_reject_encoding_failure_remains_outside_authority_invalidity() {
    assert!(matches!(
        map_recent_reject_read_error(AuthorityRecentRejectReadError::Projection),
        AuthorityDerivedError::Authority(AuthorityServiceError::Integrity(
            AuthorityIntegrityFault::Projection(AuthorityProjectionFault::Effect)
        ))
    ));
    assert!(matches!(
        map_recent_reject_read_error(AuthorityRecentRejectReadError::Encoding(
            RecentRejectEncodingError::FixedFallbackExceedsBound,
        )),
        AuthorityDerivedError::External(_)
    ));
}

#[test]
fn uak_candidate_uncle_degradation_remains_outside_authority_invalidity() {
    record_candidate_uncle_observation(Err(AuthorityTemplateDriverFault::Candidate(
        CandidateUncleMutationError::SourceVersionExhausted,
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_internal_plug_reuses_membership_without_publication_or_displacement() {
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1);
    let (assembly, _snapshot, relay) = service_assembly_with_config(config).await;
    let shared_input = OutPoint::new(Byte32::new([0x71; 32]), 0);
    let original = TransactionBuilder::default()
        .version(1u32)
        .input(CellInput::new(shared_input.clone(), 0))
        .build();
    let original_hash = original.hash();
    assembly
        .service
        .plug_entry(
            vec![TxEntry::dummy_resolve(
                original.clone(),
                11,
                Capacity::shannons(100),
                100,
            )],
            PlugTarget::Proposed,
        )
        .await
        .expect("the sealed internal fixture enters ordinary membership");
    assert_eq!(
        assembly
            .service
            .pool_summary()
            .await
            .expect("the accepted projection remains coherent")
            .proposed_size,
        1
    );
    let packed = assembly
        .service
        .package_transactions(Some(100))
        .expect("the pure authority packer consumes the same membership");
    assert_eq!(packed.len(), 1);
    assert_eq!(packed[0].transaction().hash(), original_hash);
    assert!(relay.try_recv().is_none());

    // Duplicate injection is a true no-op and cannot publish an acceptance.
    assembly
        .service
        .plug_entry(
            vec![TxEntry::dummy_resolve(
                original,
                11,
                Capacity::shannons(100),
                100,
            )],
            PlugTarget::Proposed,
        )
        .await
        .expect("an exact duplicate is ignored");
    assert!(relay.try_recv().is_none());

    // Even when ordinary RBF policy would select the original as a victim,
    // the synthetic test hook has no replacement/eviction capability.
    let rival = TransactionBuilder::default()
        .version(2u32)
        .input(CellInput::new(shared_input, 0))
        .build();
    let rejected = assembly
        .service
        .plug_entry(
            vec![TxEntry::dummy_resolve(
                rival,
                12,
                Capacity::shannons(1_000_000),
                100,
            )],
            PlugTarget::Proposed,
        )
        .await;
    assert!(matches!(
        rejected,
        Err(ckb_types::core::tx_pool::Reject::RBFRejected(_))
    ));
    let ids = assembly
        .service
        .pool_ids()
        .await
        .expect("the rejected fixture changed no owner");
    assert_eq!(ids.proposed, vec![original_hash]);
    assert!(relay.try_recv().is_none());

    assert_eq!(
        assembly.generation.shutdown(Duration::from_secs(2)).await,
        AuthorityShutdownOutcome::PersistenceEligible
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_service_persists_one_coherent_authority_receipt_outside_the_guard() {
    let directory = tempfile::TempDir::new().expect("persistence fixture directory is available");
    let mut config = runtime_config();
    config.persisted_data = directory.path().join("tx_pool");
    let read_config = config.clone();
    let snapshot = genesis_snapshot();
    let (bootstrap, _relay) = AuthorityService::prepare(config, snapshot)
        .expect("the relay handoff is constructed before service startup");
    let (verification_control, _command_tx) =
        AuthorityVerificationControl::channel(ChunkCommand::Resume);
    let (_chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let assembly = AuthorityService::assemble(
        &handle,
        AuthorityServiceInputs {
            bootstrap,
            block_assembler: None,
            verification_cache: Arc::new(RwLock::new(init_cache())),
            callbacks: Callbacks::new(),
            network: Arc::new(DummyTxPoolNetwork),
            persistence_writer: Arc::new(crate::persisted::PersistenceWriter::default()),
            recent_reject: None,
            fee_estimator: FeeEstimator::new_dummy(),
            chain_control_receiver,
            verification_control,
            cancel: CancellationToken::new(),
        },
    )
    .await
    .expect("the service generation assembles");

    assembly
        .service
        .save_pool()
        .await
        .expect("a coherent empty receipt is persisted");
    let persisted = crate::persisted::load_persistence_snapshot(&read_config)
        .expect("the just-written snapshot is readable");
    assert!(persisted.accepted.is_empty());
    assert!(persisted.recovery.is_empty());
    assert_eq!(
        assembly
            .service
            .replay_persisted(persisted)
            .await
            .expect("empty startup replay is a closed no-op"),
        super::super::service::AuthorityPersistenceReplay::default()
    );
    assert_eq!(
        assembly.generation.shutdown(Duration::from_secs(2)).await,
        AuthorityShutdownOutcome::PersistenceEligible
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_service_boundary_preserves_direct_mutation_and_read_only_semantics() {
    let recent_reject_dir = tempfile::Builder::new()
        .tempdir()
        .expect("the recent-reject fixture directory is available");
    let recent_reject = Arc::new(
        RecentReject::build(recent_reject_dir.path(), 1, 100, -1)
            .expect("the recent-reject fixture store opens"),
    );
    let (assembly, _snapshot, _relay) =
        service_assembly_with_config_and_recent_reject(runtime_config(), Some(recent_reject)).await;

    let malformed = TransactionBuilder::default().version(1u32).build();
    let test_result = assembly
        .service
        .test_accept(malformed.clone())
        .await
        .expect("read-only validation has no service fault");
    assert!(test_result.is_err());
    assert_eq!(
        assembly
            .service
            .pool_summary()
            .await
            .expect("the empty projection is coherent")
            .pending_size,
        0
    );

    let local_result = assembly
        .service
        .submit_local(malformed.clone())
        .await
        .expect("Local rejection commits through the effect authority");
    assert!(local_result.is_err());
    assert!(
        assembly
            .service
            .recent_reject_record(&malformed.hash())
            .expect("pending and persisted rejection views form one coherent surface")
            .is_some()
    );

    assert_eq!(
        assembly.generation.shutdown(Duration::from_secs(2)).await,
        AuthorityShutdownOutcome::PersistenceEligible
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_service_relay_receiver_drains_the_committed_effect_stream_directly() {
    let (assembly, _snapshot, relay) = service_assembly().await;
    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    assembly
        .service
        .submit_remote(malformed, 0, PeerIndex::from(41))
        .await
        .expect("malformed Remote ingress commits its terminal disposition");
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(result) = relay.try_recv() {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the sole relay receiver observes the committed effect");
    assert!(matches!(result, TxVerificationResult::GenerationReset));

    assert_eq!(
        assembly.generation.shutdown(Duration::from_secs(2)).await,
        AuthorityShutdownOutcome::PersistenceEligible
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_service_boundary_closes_administration_and_ordered_chain_commands() {
    let (assembly, snapshot, _relay) = service_assembly().await;
    assert!(
        !assembly
            .service
            .remove_local(&Byte32::zero())
            .await
            .expect("absent local removal is a deterministic no-op")
    );
    assembly
        .service
        .clear_pipeline()
        .await
        .expect("pipeline clear commits without a drain protocol");
    assembly
        .service
        .apply_chain_update((
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            Arc::clone(&snapshot),
        ))
        .await
        .expect("the ordered command binds and commits one paired chain cut");
    assembly
        .service
        .clear_pool(snapshot)
        .await
        .expect("pool clear installs its supplied snapshot atomically");
    assert_eq!(
        assembly.generation.shutdown(Duration::from_secs(2)).await,
        AuthorityShutdownOutcome::PersistenceEligible
    );
}
