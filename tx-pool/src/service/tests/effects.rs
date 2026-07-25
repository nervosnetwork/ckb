use super::*;
use crate::callback::Callbacks;
use crate::component::effect_outbox::EffectOutboxUsage;
use crate::component::entry::TxEntry;
use crate::network::{DummyTxPoolNetwork, TxPoolNetwork};
use ckb_types::bytes::Bytes;
use ckb_types::core::cell::CellMetaBuilder;
use ckb_types::core::{Capacity, TransactionBuilder, cell::ResolvedTransaction};
use ckb_types::packed::{CellInput, CellOutput, OutPoint};
use ckb_types::prelude::{Builder, Entity};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

impl EffectQueue {
    pub(crate) fn new(max_batches: usize, max_bytes: usize) -> Result<Self, EffectOutboxError> {
        Self::new_with_critical_capacity(max_batches, max_bytes, 0, 0)
    }

    pub(crate) async fn enqueue(
        self: &Arc<Self>,
        batch: EffectBatch,
    ) -> Result<(), EffectQueueError> {
        let permit = self.reserve(batch.charge_bytes).await?;
        permit.commit(batch)
    }

    pub(crate) fn usage(&self) -> EffectOutboxUsage {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbox
            .usage()
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            let quiescent = self.quiescent.notified();
            tokio::pin!(quiescent);
            quiescent.as_mut().enable();
            if self.usage().batches == 0 {
                return;
            }
            quiescent.await;
        }
    }
}

fn endpoints(tx_relay_sender: ckb_channel::Sender<TxVerificationResult>) -> EffectEndpoints {
    EffectEndpoints {
        network: Arc::new(DummyTxPoolNetwork),
        tx_relay_sender,
        failure_cancel: CancellationToken::new(),
    }
}

fn entry() -> TxEntry {
    TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        0,
        Capacity::zero(),
        0,
    )
}

#[test]
fn stable_effect_hash_detaches_from_transaction_backing() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(Byte32::new([9; 32]), 0), 0))
        .witness(Bytes::from(vec![1; 64 * 1024]))
        .build();
    let backing = tx.data();
    let backing_start = backing.as_slice().as_ptr() as usize;
    let backing_end = backing_start + backing.as_slice().len();
    let shared_hash = tx.input_pts_iter().next().unwrap().tx_hash();
    let shared_start = shared_hash.as_slice().as_ptr() as usize;
    assert!(shared_start >= backing_start && shared_start < backing_end);

    let batch = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: shared_hash,
    })])
    .unwrap();
    let TxPoolEffect::Relay(TxVerificationResult::Reject { tx_hash }) = &batch.effects[0] else {
        panic!("expected relay rejection")
    };
    let stored_start = tx_hash.as_slice().as_ptr() as usize;
    assert!(stored_start < backing_start || stored_start >= backing_end);
}

#[test]
fn pool_mutation_effect_formula_covers_every_generated_reject_shape() {
    let hash = Byte32::new([0xff; 32]);
    let out_point = OutPoint::new(hash.clone(), u32::MAX);
    let rejects = [
        Reject::Full(format!(
            "tx-pool total_tx_size {} overflows by add {}",
            usize::MAX,
            usize::MAX
        )),
        Reject::ExceededMaximumAncestorsCount,
        Reject::Resolve(OutPointError::Dead(out_point)),
        Reject::Resolve(OutPointError::InvalidHeader(hash.clone())),
        Reject::Expiry(u64::MAX),
        Reject::RBFRejected(format!("replaced by tx {hash}")),
        Reject::Invalidated(format!("invalidated by tx {hash}")),
    ];
    let transaction_bytes = minimum_serialized_transaction_bytes();
    let one_event_bound = max_pool_mutation_effect_bytes(transaction_bytes);
    for reject in rejects {
        assert!(bounded_pool_mutation_reject(&reject), "{reject}");
        let actual_worst_case = transaction_bytes
            .saturating_add(CALLBACK_SNAPSHOT_OVERHEAD_BYTES)
            .saturating_add(EFFECT_ENVELOPE_BYTES.saturating_mul(3))
            .saturating_add(reject.to_string().len().saturating_mul(2));
        assert!(
            actual_worst_case <= one_event_bound,
            "{reject}: {actual_worst_case} > {one_event_bound}"
        );
    }
}

#[test]
fn callback_effect_drops_resolved_payload_at_journal_boundary() {
    let transaction = TransactionBuilder::default().build();
    let transaction_bytes = transaction.data().serialized_size_in_block();
    let resolved = Arc::new(ResolvedTransaction {
        transaction: transaction.clone(),
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(
                CellOutput::new_builder().build(),
                Bytes::from(vec![0x5a; 1_000_000]),
            )
            .build(),
        ],
        resolved_inputs: Vec::new(),
        resolved_dep_groups: Vec::new(),
    });
    let resolved_owner = Arc::downgrade(&resolved);
    let entry = TxEntry::new(resolved, 42, Capacity::shannons(7), transaction_bytes);

    let effect = callback_accept(Arc::new(Callbacks::new()), entry, Status::Pending);

    assert!(
        resolved_owner.upgrade().is_none(),
        "the effect outbox must not retain resolved cell metadata"
    );
    assert!(
        effect.charge_bytes() < 4096,
        "callback charge must describe the compact snapshot, not the 1 MB resolved payload"
    );
    match effect {
        TxPoolEffect::Callback {
            event: CallbackEvent::Pending(snapshot),
            ..
        } => {
            assert_eq!(snapshot.transaction(), &transaction);
            assert_eq!(snapshot.cycles, 42);
            assert_eq!(snapshot.fee, Capacity::shannons(7));
        }
        _ => panic!("unexpected effect variant"),
    }
}

#[test]
fn submit_effect_formula_covers_coordinator_settlement_and_bounded_ban() {
    let records = 3;
    let bound = max_submit_effect_bytes(0, minimum_serialized_transaction_bytes(), records);
    let mut effects = (0..records)
        .map(|seed| {
            TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: Byte32::new([seed as u8; 32]),
            })
        })
        .collect::<Vec<_>>();
    effects.push(TxPoolEffect::BanPeer {
        peer: 1.into(),
        duration: Duration::from_secs(1),
        reason: "x".repeat(MAX_COMMIT_BAN_REASON_BYTES),
    });
    let actual = EffectBatch::new(effects).unwrap().charge_bytes;
    assert!(actual <= bound, "{actual} > {bound}");

    let reject = Reject::Malformed("test".to_string(), "界".repeat(1024));
    let reason = bounded_commit_ban_reason(&reject);
    assert!(reason.len() <= MAX_COMMIT_BAN_REASON_BYTES);
    assert!(reason.is_char_boundary(reason.len()));
}

#[test]
fn unknown_parent_effect_charges_hash_table_residency() {
    let parents = HashSet::from([Byte32::new([0; 32]), Byte32::new([0xff; 32])]);
    let effect = TxPoolEffect::Relay(TxVerificationResult::UnknownParents {
        peer: 1.into(),
        parents,
    });
    assert_eq!(
        effect.charge_bytes(),
        EFFECT_ENVELOPE_BYTES + 2 * UNKNOWN_PARENT_HASH_BYTES
    );
}

#[test]
fn oversized_recent_reject_diagnostic_is_bounded_without_changing_original() {
    let reject = Reject::Full("界".repeat(2_000));
    let bounded = bounded_recent_reject(&reject);
    assert!(bounded.to_string().len() <= MAX_RECENT_REJECT_BYTES);
    assert!(serialized_recent_reject(&reject).len() <= MAX_RECENT_REJECT_BYTES);
    assert!(reject.to_string().len() > MAX_RECENT_REJECT_BYTES);
}

#[tokio::test]
async fn recent_reject_database_write_runs_as_a_journaled_effect() {
    let temp = tempfile::Builder::new().tempdir().unwrap();
    let store = Arc::new(
        crate::component::recent_reject::RecentReject::build(temp.path(), 1, 100, -1).unwrap(),
    );
    let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
    let tx_hash = Byte32::new([0x42; 32]);
    queue
        .enqueue(
            EffectBatch::new(vec![TxPoolEffect::RecentReject {
                store: Arc::clone(&store),
                tx_hash: tx_hash.clone(),
                serialized: serialized_recent_reject(&Reject::Expiry(42)),
            }])
            .unwrap(),
        )
        .await
        .unwrap();
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        endpoints(relay_tx),
    ));
    tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
        .await
        .unwrap();
    assert!(store.get(&tx_hash).unwrap().is_some());
    queue.close();
    publisher.await.unwrap();
}

#[tokio::test]
async fn full_relayer_retains_fifo_head_and_outbox_charge() {
    let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::Reject {
            tx_hash: Byte32::zero(),
        })
        .unwrap();
    let expected = Byte32::new([7; 32]);
    queue
        .enqueue(
            EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: expected.clone(),
            })])
            .unwrap(),
        )
        .await
        .unwrap();
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        endpoints(relay_tx),
    ));
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(queue.usage().batches, 1);
    relay_rx.try_recv().unwrap();

    tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
        .await
        .unwrap();
    match relay_rx.try_recv().unwrap() {
        TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected),
        other => panic!("unexpected relay result: {other:?}"),
    }
    queue.close();
    publisher.await.unwrap();
}

#[tokio::test]
async fn close_drains_every_queued_batch_in_order() {
    let queue = Arc::new(EffectQueue::new(4, 1_000_000).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded(4);
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    for value in [1usize, 2] {
        let mut callbacks = Callbacks::new();
        let observed = Arc::clone(&order);
        callbacks.register_pending(Box::new(move |_| {
            observed.lock().unwrap().push(value);
        }));
        queue
            .enqueue(
                EffectBatch::new(vec![callback_accept(
                    Arc::new(callbacks),
                    entry(),
                    Status::Pending,
                )])
                .unwrap(),
            )
            .await
            .unwrap();
    }
    queue.close();
    run_effect_publisher(Arc::clone(&queue), endpoints(relay_tx)).await;
    assert_eq!(*order.lock().unwrap(), vec![1, 2]);
    assert_eq!(queue.usage().batches, 0);
}

#[tokio::test]
async fn panicking_endpoint_is_quarantined_once_without_blocking_fifo() {
    struct PanickingNetwork {
        attempts: Arc<AtomicUsize>,
    }

    impl TxPoolNetwork for PanickingNetwork {
        fn ban_peer(&self, _peer: PeerIndex, _duration: Duration, _reason: String) {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            panic!("injected permanent network endpoint panic");
        }
    }

    let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    let attempts = Arc::new(AtomicUsize::new(0));
    let expected = Byte32::new([9; 32]);
    queue
        .enqueue(
            EffectBatch::new(vec![
                TxPoolEffect::BanPeer {
                    peer: 1.into(),
                    duration: Duration::from_secs(1),
                    reason: "injected".to_owned(),
                },
                TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: expected.clone(),
                }),
            ])
            .unwrap(),
        )
        .await
        .unwrap();

    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        EffectEndpoints {
            network: Arc::new(PanickingNetwork {
                attempts: Arc::clone(&attempts),
            }),
            tx_relay_sender: relay_tx,
            failure_cancel: CancellationToken::new(),
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
        .await
        .expect("a permanently panicking endpoint must not retain the FIFO head");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    match relay_rx.try_recv().unwrap() {
        TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected),
        other => panic!("unexpected relay result: {other:?}"),
    }

    queue.close();
    publisher.await.unwrap();
}

#[tokio::test]
async fn publisher_invariant_failure_closes_outbox_and_cancels_service() {
    let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
    queue
        .enqueue(
            EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: Byte32::zero(),
            })])
            .unwrap(),
        )
        .await
        .unwrap();
    // Simulate a second/failed publisher that left an active checkout.
    // The production publisher must fail the complete service instead of
    // logging and waiting forever behind that impossible state.
    {
        let mut state = queue.state.lock().unwrap();
        assert!(state.outbox.checkout().unwrap().is_some());
    }

    let failure_cancel = CancellationToken::new();
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        EffectEndpoints {
            network: Arc::new(DummyTxPoolNetwork),
            tx_relay_sender: relay_tx,
            failure_cancel: failure_cancel.clone(),
        },
    ));
    let result = tokio::time::timeout(Duration::from_secs(1), publisher)
        .await
        .expect("fatal publisher invariant must not hang");
    assert!(result.is_err());
    assert!(failure_cancel.is_cancelled());
    assert!(matches!(
        queue.reserve(1).await,
        Err(EffectQueueError::Closed)
    ));
}

#[tokio::test]
async fn unused_pre_mutation_reservation_is_refunded_on_cancellation() {
    let queue = Arc::new(EffectQueue::new(1, 100).unwrap());
    let permit = queue.reserve(100).await.unwrap();
    assert_eq!(queue.usage().batches, 1);
    drop(permit);
    assert_eq!(queue.usage().batches, 0);

    // The exact same full-capacity reservation must be available again;
    // no abandoned async operation can strand an outbox credit.
    let permit = queue.reserve(100).await.unwrap();
    drop(permit);
    assert_eq!(queue.usage().batches, 0);
}

#[tokio::test]
async fn close_wakes_every_blocked_capacity_waiter() {
    let queue = Arc::new(EffectQueue::new(1, 100).unwrap());
    let full = queue.reserve(100).await.unwrap();
    let first_queue = Arc::clone(&queue);
    let second_queue = Arc::clone(&queue);
    let first = tokio::spawn(async move { first_queue.reserve(1).await });
    let second = tokio::spawn(async move { second_queue.reserve(1).await });
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());

    queue.close();
    for waiter in [first, second] {
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("close must wake every registered capacity waiter")
            .unwrap();
        assert!(matches!(result, Err(EffectQueueError::Closed)));
    }
    drop(full);
}

#[tokio::test]
async fn idle_publisher_observes_close_without_a_later_ready_event() {
    let queue = Arc::new(EffectQueue::new(1, 100).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        endpoints(relay_tx),
    ));
    tokio::task::yield_now().await;
    queue.close();
    tokio::time::timeout(Duration::from_secs(1), publisher)
        .await
        .expect("an idle publisher must observe close without another batch")
        .unwrap();
}

#[tokio::test]
async fn binding_conservative_reservation_wakes_byte_capacity_waiter() {
    let queue = Arc::new(EffectQueue::new(2, 1_000).unwrap());
    let first = queue.reserve(1_000).await.unwrap();
    let waiting_queue = Arc::clone(&queue);
    let waiter = tokio::spawn(async move { waiting_queue.reserve(872).await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    first
        .commit(
            EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: Byte32::zero(),
            })])
            .unwrap(),
        )
        .unwrap();

    let second = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("reservation shrink must wake the waiter")
        .unwrap();
    drop(second);
}

#[tokio::test]
async fn ordinary_backpressure_cannot_consume_critical_reorg_headroom() {
    let queue = Arc::new(EffectQueue::new_with_critical_capacity(1, 128, 1, 1_000).unwrap());
    let ordinary = queue.reserve(128).await.unwrap();
    let waiting_queue = Arc::clone(&queue);
    let ordinary_waiter = tokio::spawn(async move { waiting_queue.reserve(1).await });
    tokio::task::yield_now().await;
    assert!(!ordinary_waiter.is_finished());

    let critical = tokio::time::timeout(Duration::from_secs(1), queue.reserve_critical(1_000))
        .await
        .expect("ordinary saturation must leave critical headroom")
        .unwrap();

    drop(critical);
    drop(ordinary);
    let waiter = tokio::time::timeout(Duration::from_secs(1), ordinary_waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(waiter);
}
