use super::*;

/// A proposal short ID identifies only one protocol slot, not transaction
/// equality. Reporting a distinct colliding remote transaction as `Duplicated`
/// suppresses the relayer Reject terminal and leaves its filter resident. The
/// admission adapter must expose retryable backpressure and settle it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_short_id_collision_is_not_a_successful_duplicate() {
    use crate::component::entry::TxEntry;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x24; 32];
    let mut incoming_hash = accepted_hash;
    accepted_hash[31] = 1;
    incoming_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let incoming = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(incoming_hash),
    );
    assert_eq!(accepted.proposal_short_id(), incoming.proposal_short_id());
    assert_ne!(accepted.hash(), incoming.hash());
    let incoming_hash = incoming.hash();
    let accepted_id = accepted.proposal_short_id();
    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(accepted, 0, Capacity::zero(), 100),
            Status::Pending,
        )
        .unwrap();

    let peer = ckb_network::PeerIndex::from(29);
    let reject = h
        .service
        .submit_remote_tx(incoming, TxSource::Remote { cycles: 0, peer })
        .await
        .expect_err("the occupied proposal slot must reject the distinct hash");
    assert!(matches!(reject, crate::error::Reject::Full(_)));
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_pool_entry(&accepted_id)
            .is_some()
    );
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&incoming_hash))
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the rejected remote ingress must release its relayer filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == incoming_hash
    ));

    h.cancel.cancel();
}

/// Synchronous Local/reorg submissions use `pre_check` before the shared
/// authoritative commit. They must apply the same full-hash identity rule as
/// remote admission: an occupied short-id slot is retryable backpressure, not
/// evidence that the distinct transaction was already accepted.
#[tokio::test]
async fn synchronous_precheck_does_not_alias_short_id_collision_as_duplicate() {
    use crate::component::entry::TxEntry;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x25; 32];
    let mut incoming_hash = accepted_hash;
    accepted_hash[31] = 1;
    incoming_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let incoming = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(incoming_hash),
    );
    assert_eq!(accepted.proposal_short_id(), incoming.proposal_short_id());
    assert_ne!(accepted.hash(), incoming.hash());
    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(accepted, 0, Capacity::zero(), 100),
            Status::Pending,
        )
        .unwrap();

    let tx_size = incoming.data().serialized_size_in_block();
    let (result, _) = h.service.pre_check(&incoming, tx_size).await;
    assert!(matches!(result, Err(crate::error::Reject::Full(_))));

    h.cancel.cancel();
}

pub(super) async fn measured_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    service
        .test_accept_tx(tx)
        .await
        .expect("local test accept should succeed")
        .cycles
}

// -----------------------------------------------------------------------------
// secp256k1 1-in-1-out helpers
// -----------------------------------------------------------------------------

const SECP_PRIVKEY: H256 =
    h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const SECP_PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
// 50,000 CKB and 1,000 CKB expressed in shannons (1 CKB = 10^8 shannons).
pub(super) const SECP_ISSUE_CAPACITY: u64 = 50_000 * 100_000_000;
pub(super) const SECP_FEE: u64 = 1_000 * 100_000_000;

pub(super) fn secp_script() -> Script {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_blake160_sighash_all")
        .expect("load secp256k1_blake160_sighash_all");
    let data: Bytes = raw_data.to_vec().into();
    Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&data))
        .args(Bytes::from(SECP_PUBKEY_HASH.as_bytes()))
        .hash_type(ckb_types::core::ScriptHashType::Data)
        .build()
}

pub(super) fn secp_data_cell() -> (CellOutput, Bytes) {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_data")
        .expect("load secp256k1_data");
    let data: Bytes = raw_data.to_vec().into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .build();
    (cell, data)
}

pub(super) fn secp_code_cell() -> (CellOutput, Bytes) {
    let raw_data = BUNDLED_CELL
        .get("specs/cells/secp256k1_blake160_sighash_all")
        .expect("load secp256k1_blake160_sighash_all");
    let data: Bytes = raw_data.to_vec().into();
    let cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .build();
    (cell, data)
}

pub(super) fn create_secp_system_tx() -> TransactionView {
    let (code_cell, code_data) = secp_code_cell();
    let (data_cell, data_data) = secp_data_cell();
    let (_, _, script) = (code_cell.clone(), code_data.clone(), secp_script());
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![code_cell, data_cell])
        .outputs_data(vec![code_data.pack(), data_data.pack()])
        .witness(script.into_witness())
        .build()
}

pub(super) fn secp_cell_deps(system_tx: &TransactionView) -> Vec<CellDep> {
    vec![
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 0))
            .build(),
        CellDep::new_builder()
            .out_point(OutPoint::new(system_tx.hash(), 1))
            .build(),
    ]
}

pub(crate) fn secp_test_consensus(
    issue_outputs: usize,
) -> (Consensus, Vec<OutPoint>, Vec<CellDep>) {
    let system_tx = create_secp_system_tx();
    let cell_deps = secp_cell_deps(&system_tx);
    let lock = secp_script();

    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::shannons(SECP_ISSUE_CAPACITY))
        .lock(lock)
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();

    let issue_out_points: Vec<_> = (0..issue_outputs)
        .map(|i| OutPoint::new(issue_tx.hash(), i as u32))
        .collect();

    let dao = ckb_dao_utils::genesis_dao_data(vec![&system_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(system_tx)
        .transaction(issue_tx)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_out_points, cell_deps)
}

pub(super) fn secp_service_with_pipeline_workers(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
    Vec<CellDep>,
) {
    let h = crate::component::tests::harness::harness(issue_outputs)
        .secp(true)
        .max_workers(max_workers)
        .build();
    (
        h.service,
        h.relay_rx,
        h.cancel,
        h.store,
        h.out_points,
        h.cell_deps.expect("secp harness provides cell deps"),
    )
}

pub(super) fn build_secp_tx(
    input: &OutPoint,
    cell_deps: &[CellDep],
    output_capacity: u64,
) -> TransactionView {
    let lock = secp_script();
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(output_capacity))
        .lock(lock)
        .build();

    let raw = TransactionBuilder::default()
        .inputs(vec![CellInput::new(input.clone(), 0)])
        .cell_deps(cell_deps.iter().cloned())
        .output(output)
        .output_data(Bytes::default().pack())
        .build();

    let privkey: Privkey = SECP_PRIVKEY.into();
    let witness_placeholder = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])))
        .build();
    let witness_len: u64 = witness_placeholder.as_bytes().len() as u64;

    let mut blake2b = ckb_hash::new_blake2b();
    let mut message = [0u8; 32];
    blake2b.update(&raw.hash().raw_data()[..]);
    blake2b.update(&witness_len.to_le_bytes());
    blake2b.update(&witness_placeholder.as_bytes());
    blake2b.finalize(&mut message);

    let message = H256::from(message);
    let sig: Bytes = privkey
        .sign_recoverable(&message)
        .expect("sign tx")
        .serialize()
        .into();
    let witness = witness_placeholder.as_builder().lock(Some(sig)).build();

    raw.as_advanced_builder()
        .set_witnesses(vec![witness.as_bytes().into()])
        .build()
}

pub(super) async fn submit_local_tx(service: &TxPoolService, tx: TransactionView) -> u64 {
    service
        .process_tx_direct(tx, TxSource::Local, None)
        .await
        .expect("local tx should be accepted")
        .cycles
}

pub(super) async fn verify_cycles(service: &TxPoolService, tx: TransactionView) -> u64 {
    let tx_size = tx.data().serialized_size_in_block();
    let (pre_check_ret, snapshot) = service.pre_check(&tx, tx_size).await;
    let PreCheckedTx { rtx, status, .. } =
        pre_check_ret.expect("pre_check for cycle measurement should succeed");
    let verify_cache = service.fetch_tx_verify_cache(&tx).await;
    let max_cycles = service.pool.consensus.max_block_cycles();
    let tx_env = match status {
        Status::Pending => Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
        Status::Gap => Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 0)),
        Status::Proposed => Arc::new(TxVerifyEnv::new_proposed(snapshot.tip_header(), 1)),
    };
    let verified = crate::util::verify_rtx(
        Arc::clone(&snapshot),
        Arc::clone(&rtx),
        tx_env,
        &verify_cache,
        max_cycles,
        None,
    )
    .await
    .expect("verify_rtx for cycle measurement should succeed");
    verified.cycles
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verification_cache_isolated_by_witness_hash_not_raw_hash() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let raw = build_tx(&issue_out_points[0], 4_000);
    let first = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(first.hash(), second.hash());
    assert_ne!(first.witness_hash(), second.witness_hash());

    let cached = Completed {
        cycles: 42,
        fee: Capacity::shannons(7),
    };
    service
        .aux
        .txs_verify_cache
        .write()
        .await
        .put(TxVerificationCacheKey::from_transaction(&first), cached);

    assert_eq!(service.fetch_tx_verify_cache(&first).await, Some(cached));
    assert_eq!(service.fetch_tx_verify_cache(&second).await, None);
    signal.cancel();
}

/// Detached-transaction recovery must query the verification cache with the
/// exact witness-bearing transaction being recovered.  The historical
/// `readd_detached_tx` path built a map by witness hash but looked it up by raw
/// transaction hash, turning every recovery into an avoidable cache miss.
/// This also guards against the more dangerous inverse error: reusing a cache
/// entry produced for another witness variant with the same raw hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorg_recovery_reads_cache_by_exact_witness_hash() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(2);
    let exact = build_tx(&issue_out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"exact").pack()])
        .build();
    let other_raw = build_tx(&issue_out_points[1], 4_000);
    let cached_variant = other_raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"cached-variant").pack()])
        .build();
    let recovered_variant = other_raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"recovered-variant").pack()])
        .build();
    assert_eq!(cached_variant.hash(), recovered_variant.hash());
    assert_ne!(
        cached_variant.witness_hash(),
        recovered_variant.witness_hash()
    );

    let exact_measured = verify_cycles(&service, exact.clone()).await;
    let recovered_measured = verify_cycles(&service, recovered_variant.clone()).await;
    let exact_cached = exact_measured + 17;
    let wrong_variant_cached = recovered_measured + 29;
    assert!(wrong_variant_cached < MAX_TX_VERIFY_CYCLES);

    {
        let mut cache = service.aux.txs_verify_cache.write().await;
        cache.put(
            TxVerificationCacheKey::from_transaction(&exact),
            Completed {
                cycles: exact_cached,
                fee: Capacity::shannons(0),
            },
        );
        cache.put(
            TxVerificationCacheKey::from_transaction(&cached_variant),
            Completed {
                cycles: wrong_variant_cached,
                fee: Capacity::shannons(0),
            },
        );
    }

    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1_000).unwrap())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build(),
        )
        .transaction(exact.clone())
        .transaction(recovered_variant.clone())
        .build();
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    service
        .update_tx_pool_for_reorg(
            VecDeque::from([detached_block]),
            VecDeque::new(),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();

    let pool = service.pool.tx_pool.read().await;
    let exact_entry = pool
        .get_pool_entry(&exact.proposal_short_id())
        .expect("exact cached detached transaction is recovered");
    assert_eq!(
        exact_entry.inner.cycles, exact_cached,
        "detached recovery must hit the exact witness-hash cache entry"
    );
    let recovered_entry = pool
        .get_pool_entry(&recovered_variant.proposal_short_id())
        .expect("uncached witness variant is recovered");
    assert_eq!(
        recovered_entry.inner.cycles, recovered_measured,
        "a cache entry for another witness variant must not be reused"
    );
    drop(pool);

    signal.cancel();
}

/// Script verification needs complete expanded cell deps, but retaining their
/// attacker-controlled outputs/data after verification turns a compact
/// dep-group reference into an accepted-pool memory multiplier. The typed
/// verified-to-candidate transition strips that verification-only payload,
/// preserves the dep identity/maturity metadata, and retains complete inputs
/// for DAO accounting. The accepted-pool resident budget still accounts for
/// the payload that must remain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_candidate_compacts_deps_and_pool_budget_counts_retained_inputs() {
    use crate::component::entry::{
        TxEntry, accepted_transaction_charge_bytes, resolved_transaction_charge_bytes,
    };
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::resolved_tx::ResolvedTx;
    use ckb_types::core::TransactionInfo;
    use ckb_types::core::cell::{CellMetaBuilder, ResolvedTransaction};

    let h = harness(1).workers(WorkerSet::None).build();
    let transaction = build_tx(&h.out_points[0], 4_000);
    let tx_size = transaction.data().serialized_size_in_block();
    let retained_input_data = Bytes::from(vec![0x5a; 1_000_000]);
    let dep_data = Bytes::from(vec![0xa5; 1_000_000]);
    let dep_out_point = OutPoint::new(Default::default(), 7);
    let dep_transaction_info = TransactionInfo::new(
        1,
        EpochNumberWithFraction::new(0, 0, 1),
        Default::default(),
        0,
    );
    let resolved = Arc::new(ResolvedTransaction {
        transaction: transaction.clone(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder().build(),
                retained_input_data,
            )
            .out_point(h.out_points[0].clone())
            .build(),
        ],
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(CellOutput::new_builder().build(), dep_data)
                .out_point(dep_out_point.clone())
                .transaction_info(dep_transaction_info.clone())
                .build(),
        ],
        resolved_dep_groups: Vec::new(),
    });
    let full_resident_size = resolved_transaction_charge_bytes(tx_size, &resolved);
    assert!(full_resident_size > tx_size + 2_000_000);
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let candidate = ResolvedTx {
        tx: transaction,
        rtx: resolved,
        status: Status::Pending,
        fee: Capacity::shannons(1000),
        tx_size,
        resident_size: full_resident_size,
        pre_resolve_tip: snapshot.tip_hash(),
        source: TxSource::Local,
        epoch: 0,
    }
    .into_pool_candidate();

    let dep = &candidate.rtx.resolved_cell_deps[0];
    assert_eq!(dep.out_point, dep_out_point);
    assert_eq!(dep.transaction_info, Some(dep_transaction_info));
    assert_eq!(dep.cell_output, CellOutput::default());
    assert_eq!(dep.data_bytes, 0);
    assert!(dep.mem_cell_data.is_none());
    assert!(dep.mem_cell_data_hash.is_none());
    assert_eq!(
        candidate.rtx.resolved_inputs[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        Some(1_000_000),
        "DAO-relevant input data must remain available"
    );
    assert!(candidate.resident_size > tx_size + 1_000_000);
    assert!(candidate.resident_size < full_resident_size - 900_000);
    assert_eq!(
        candidate.resident_size,
        accepted_transaction_charge_bytes(tx_size, &candidate.rtx),
        "verified handoff must reserve accepted payload and index residency"
    );
    assert!(
        candidate.resident_size > resolved_transaction_charge_bytes(tx_size, &candidate.rtx),
        "accepted-state indexes must not be omitted from the resident budget"
    );

    let entry = TxEntry::new_with_resident_size(
        candidate.rtx,
        42,
        candidate.fee,
        tx_size,
        candidate.resident_size,
    );
    let resident_size = entry.resident_size();
    let id = entry.proposal_short_id();

    let mut pool = h.service.pool.tx_pool.write().await;
    pool.config.max_tx_pool_size = tx_size;
    pool.config.max_tx_pool_resident_size = resident_size - 1;
    pool.add_pending(entry)
        .expect("entry inserts before limits run");
    assert_eq!(pool.pool_map.stats.total_tx_size, tx_size);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, resident_size);

    let mut rejects = Vec::new();
    let reject = pool.limit_size(Some(&id), &mut rejects);
    assert!(matches!(reject, Some(crate::error::Reject::Full(_))));
    assert!(pool.get_pool_entry(&id).is_none());
    assert_eq!(pool.pool_map.stats.total_tx_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, 0);
    assert_eq!(rejects.len(), 1);
    drop(pool);

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limit_size_repairs_high_counter_drift_before_returning() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let mut pool = h.service.pool.tx_pool.write().await;
    let serialized_drift = pool.config.max_tx_pool_size.saturating_add(1);
    let resident_drift = pool.config.tx_pool_resident_size_budget().saturating_add(1);
    pool.pool_map.stats.total_tx_size = serialized_drift;
    pool.pool_map.stats.total_tx_resident_size = resident_drift;

    let mut rejects = Vec::new();
    assert!(pool.limit_size(None, &mut rejects).is_none());
    assert!(rejects.is_empty());
    assert_eq!(pool.pool_map.stats.total_tx_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_resident_size, 0);
    assert_eq!(pool.pool_map.stats.total_tx_cycles, 0);
    drop(pool);
    h.cancel.cancel();
}
