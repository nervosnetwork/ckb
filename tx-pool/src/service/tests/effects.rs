use super::*;

impl EffectJournal {
    pub(crate) fn usage(&self) -> EffectUsage {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).usage[EffectClass::Critical.region()]
    }
}
use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::network::{DummyTxPoolNetwork, TxPoolNetwork};
use ckb_types::bytes::Bytes;
use ckb_types::core::cell::CellMetaBuilder;
use ckb_types::core::{Capacity, TransactionBuilder, cell::ResolvedTransaction};
use ckb_types::packed::{CellInput, CellOutput, OutPoint};
use ckb_types::prelude::{Builder, Entity};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

impl EffectJournal {
    pub(crate) fn new(max_batches: usize, max_bytes: usize) -> Result<Self, EffectJournalError> {
        Self::new_partitioned(max_batches, max_bytes, 0, 0, 0, 0)
    }

    pub(crate) async fn enqueue(
        self: &Arc<Self>,
        batch: EffectBatch,
    ) -> Result<(), EffectJournalError> {
        self.append(batch, EffectClass::Trusted).await
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            let space = self.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            let idle = {
                let state = self.state.lock().unwrap();
                state.active.is_none()
                    && state.queued.is_empty()
                    && state.latest_generation_reset.is_none()
            };
            if idle {
                return;
            }
            space.await;
        }
    }
}

fn reject_batch(seed: u8) -> EffectBatch {
    EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: Byte32::new([seed; 32]),
    })])
    .unwrap()
}

fn endpoints(tx_relay_sender: ckb_channel::Sender<TxVerificationResult>) -> EffectEndpoints {
    EffectEndpoints {
        network: Arc::new(DummyTxPoolNetwork),
        tx_relay_sender,
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
        "the effect journal must not retain resolved cell metadata"
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
    let bound = max_submit_effect_bytes(0, minimum_serialized_transaction_bytes());
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
    let queue = Arc::new(EffectJournal::new(2, 1_000_000).unwrap());
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
async fn full_relayer_retains_fifo_head_and_journal_charge() {
    let queue = Arc::new(EffectJournal::new(2, 1_000_000).unwrap());
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
    let queue = Arc::new(EffectJournal::new(4, 1_000_000).unwrap());
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
async fn generation_reset_register_bypasses_full_fifo_and_coalesces() {
    let queue = Arc::new(EffectJournal::new(2, 1_000).unwrap());
    queue
        .try_apply(Some(reject_batch(1)), EffectClass::Trusted, || ())
        .unwrap();
    queue.install_generation_reset().unwrap();
    queue
        .install_generation_reset()
        .expect("a newer reset replaces the reserved record");
    queue
        .try_apply(Some(reject_batch(2)), EffectClass::Trusted, || ())
        .unwrap();
    assert_eq!(queue.usage().batches, 2, "reset owns no FIFO budget");
    assert!(
        matches!(
            queue.try_apply(Some(reject_batch(3)), EffectClass::Trusted, || ()),
            Err(EffectJournalError::Full)
        ),
        "ordinary FIFO is saturated independently of the reset register"
    );

    let (relay_tx, relay_rx) = ckb_channel::bounded(4);
    queue.close();
    run_effect_publisher(Arc::clone(&queue), endpoints(relay_tx)).await;
    let published = (0..3)
        .map(|_| relay_rx.try_recv().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        &published[0],
        TxVerificationResult::Reject { tx_hash } if tx_hash == &Byte32::new([1; 32])
    ));
    assert!(matches!(
        published[1],
        TxVerificationResult::GenerationReset
    ));
    assert!(matches!(
        &published[2],
        TxVerificationResult::Reject { tx_hash } if tx_hash == &Byte32::new([2; 32])
    ));
    assert!(relay_rx.try_recv().is_err(), "two resets coalesce to one");
}

#[tokio::test]
async fn authoritative_apply_falls_back_to_prebuilt_reset_when_fifo_is_full() {
    let queue = Arc::new(EffectJournal::new_partitioned(1, 128, 0, 0, 1, 128).unwrap());
    queue
        .try_apply(Some(reject_batch(1)), EffectClass::Remote, || ())
        .unwrap();
    queue
        .try_apply(Some(reject_batch(2)), EffectClass::Critical, || ())
        .unwrap();
    let applied = AtomicUsize::new(0);
    queue
        .try_apply_authoritative(EFFECT_ENVELOPE_BYTES, |publish_detail| {
            assert!(!publish_detail, "the detailed critical FIFO is saturated");
            applied.fetch_add(1, Ordering::SeqCst);
            ((), None)
        })
        .expect("chain authority does not backpressure behind publication");
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    assert_eq!(queue.usage().batches, 2, "reset is outside FIFO capacity");

    let (relay_tx, relay_rx) = ckb_channel::bounded(3);
    queue.close();
    run_effect_publisher(Arc::clone(&queue), endpoints(relay_tx)).await;
    let published = (0..3)
        .map(|_| relay_rx.try_recv().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        &published[0],
        TxVerificationResult::Reject { tx_hash } if tx_hash == &Byte32::new([1; 32])
    ));
    assert!(matches!(
        &published[1],
        TxVerificationResult::Reject { tx_hash } if tx_hash == &Byte32::new([2; 32])
    ));
    assert!(matches!(
        published[2],
        TxVerificationResult::GenerationReset
    ));
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

    let queue = Arc::new(EffectJournal::new(2, 1_000_000).unwrap());
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
async fn hung_callback_opens_one_stable_circuit_and_does_not_pin_relay() {
    let queue = Arc::new(EffectJournal::new(2, 1_000_000).unwrap());
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut callbacks = Callbacks::new();
    let observed = Arc::clone(&calls);
    callbacks.register_pending(Box::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(250));
    }));
    let callbacks = Arc::new(callbacks);
    let expected = Byte32::new([0x66; 32]);
    queue
        .enqueue(
            EffectBatch::new(vec![
                callback_accept(Arc::clone(&callbacks), entry(), Status::Pending),
                callback_accept(callbacks, entry(), Status::Pending),
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
        endpoints(relay_tx),
    ));
    tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
        .await
        .expect("hung callback must not pin the journal");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(queue.callback_circuit_open.load(Ordering::Acquire));
    assert!(matches!(
        relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == expected
    ));
    queue.close();
    publisher.await.unwrap();
}

#[tokio::test]
async fn replacement_publisher_resumes_the_charged_active_batch() {
    let queue = Arc::new(EffectJournal::new(2, 1_000_000).unwrap());
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    let expected = Byte32::new([0x44; 32]);
    queue
        .enqueue(
            EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: expected.clone(),
            })])
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        queue.checkout().is_some(),
        "simulate a publisher dying after checkout"
    );

    queue.close();
    run_effect_publisher(Arc::clone(&queue), endpoints(relay_tx)).await;
    assert_eq!(queue.usage().batches, 0);
    assert!(matches!(
        relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == expected
    ));
}

#[tokio::test]
async fn capacity_wait_is_not_a_reservation_or_resident_owner() {
    let queue = EffectJournal::new(1, 100).unwrap();
    queue
        .wait_capacity(100, EffectClass::Trusted)
        .await
        .unwrap();
    assert_eq!(queue.usage(), EffectUsage::default());
}

#[tokio::test]
async fn close_wakes_every_blocked_capacity_waiter() {
    let queue = Arc::new(EffectJournal::new(1, 256).unwrap());
    queue
        .try_apply_bounded(EFFECT_ENVELOPE_BYTES, EffectClass::Trusted, || {
            (
                (),
                EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: Byte32::zero(),
                })]),
            )
        })
        .unwrap();
    let first_queue = Arc::clone(&queue);
    let second_queue = Arc::clone(&queue);
    let first =
        tokio::spawn(async move { first_queue.wait_capacity(1, EffectClass::Trusted).await });
    let second =
        tokio::spawn(async move { second_queue.wait_capacity(1, EffectClass::Trusted).await });
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());

    queue.close();
    for waiter in [first, second] {
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("close must wake every registered capacity waiter")
            .unwrap();
        assert!(matches!(result, Err(EffectJournalError::Closed)));
    }
}

#[tokio::test]
async fn idle_publisher_observes_close_without_a_later_ready_event() {
    let queue = Arc::new(EffectJournal::new(1, 100).unwrap());
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
async fn full_journal_does_not_run_the_state_apply_closure() {
    let queue = EffectJournal::new(1, 1_000).unwrap();
    queue
        .try_apply(Some(reject_batch(0)), EffectClass::Trusted, || ())
        .unwrap();
    let applied = AtomicUsize::new(0);
    assert_eq!(
        queue.try_apply(Some(reject_batch(1)), EffectClass::Trusted, || {
            applied.fetch_add(1, Ordering::SeqCst);
        }),
        Err(EffectJournalError::Full)
    );
    assert_eq!(applied.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ordinary_backpressure_cannot_consume_critical_reorg_headroom() {
    let queue = Arc::new(EffectJournal::new_partitioned(1, 128, 0, 0, 1, 1_000).unwrap());
    let remote = reject_batch(0);
    let remote_bytes = remote.charge_bytes();
    queue
        .try_apply(Some(remote), EffectClass::Remote, || ())
        .unwrap();
    let waiting_queue = Arc::clone(&queue);
    let ordinary_waiter = tokio::spawn(async move {
        waiting_queue
            .wait_capacity(remote_bytes, EffectClass::Remote)
            .await
    });
    tokio::task::yield_now().await;
    assert!(!ordinary_waiter.is_finished());

    queue
        .try_apply(Some(reject_batch(1)), EffectClass::Critical, || ())
        .expect("Remote saturation must leave trusted/critical headroom");
    queue.close();
    ordinary_waiter.await.unwrap().unwrap_err();
}

#[test]
fn journal_usage_charges_queued_and_active_batches_exactly() {
    let journal = EffectJournal::new(2, 1_000).unwrap();
    journal
        .try_apply(Some(reject_batch(0)), EffectClass::Trusted, || ())
        .unwrap();
    journal
        .try_apply(Some(reject_batch(1)), EffectClass::Trusted, || ())
        .unwrap();
    assert_eq!(
        journal.usage(),
        EffectUsage {
            batches: 2,
            bytes: 256
        }
    );
    let (sequence, _) = journal.checkout().unwrap();
    assert_eq!(
        journal.usage(),
        EffectUsage {
            batches: 2,
            bytes: 256
        }
    );
    journal.complete(sequence);
    assert_eq!(
        journal.usage(),
        EffectUsage {
            batches: 1,
            bytes: 128
        }
    );
}

#[test]
fn journal_sequence_is_total_apply_order() {
    let journal = EffectJournal::new(2, 1_000).unwrap();
    for seed in [1u8, 2] {
        let batch = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
            tx_hash: Byte32::new([seed; 32]),
        })])
        .unwrap();
        journal
            .try_apply(Some(batch), EffectClass::Trusted, || ())
            .unwrap();
    }
    let state = journal.state.lock().unwrap();
    assert_eq!(
        state
            .queued
            .iter()
            .map(|envelope| envelope.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn journal_accounting_drift_fails_fast_instead_of_repairing() {
    let journal = EffectJournal::new(1, 1_000).unwrap();
    journal
        .try_apply(Some(reject_batch(0)), EffectClass::Trusted, || ())
        .unwrap();
    let (sequence, _) = journal.checkout().expect("batch becomes active");
    journal.state.lock().unwrap().usage[EffectClass::Trusted.region()].batches = 0;

    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| journal.complete(sequence)))
            .is_err(),
        "an internal accounting contradiction must not be hidden by recomputation"
    );
}

#[test]
fn remote_byte_ceiling_cannot_borrow_trusted_headroom() {
    let journal = EffectJournal::new_partitioned(2, 128, 1, 128, 1, 128).unwrap();
    journal
        .try_apply(Some(reject_batch(0)), EffectClass::Remote, || ())
        .unwrap();
    assert_eq!(
        journal.try_apply(Some(reject_batch(1)), EffectClass::Remote, || ()),
        Err(EffectJournalError::Full)
    );
    journal
        .try_apply(Some(reject_batch(2)), EffectClass::Trusted, || ())
        .expect("trusted headroom is a separate byte partition");
}

#[test]
fn trusted_saturation_cannot_consume_critical_headroom() {
    let journal = EffectJournal::new_partitioned(1, 128, 1, 128, 1, 128).unwrap();
    journal
        .try_apply(Some(reject_batch(0)), EffectClass::Remote, || ())
        .unwrap();
    journal
        .try_apply(Some(reject_batch(1)), EffectClass::Trusted, || ())
        .unwrap();
    journal
        .try_apply(Some(reject_batch(2)), EffectClass::Critical, || ())
        .expect("critical authority has independent capacity");
}

#[test]
fn active_critical_batch_does_not_consume_ordinary_headroom() {
    let journal = EffectJournal::new_partitioned(1, 128, 1, 128, 1, 128).unwrap();
    journal
        .try_apply(Some(reject_batch(0)), EffectClass::Critical, || ())
        .unwrap();
    journal
        .try_apply(Some(reject_batch(1)), EffectClass::Remote, || ())
        .unwrap();
    journal
        .try_apply(Some(reject_batch(2)), EffectClass::Trusted, || ())
        .unwrap();
    assert_eq!(journal.usage().batches, 3);
}

#[tokio::test]
async fn full_relayer_coalesces_to_bounded_reconciliation() {
    let journal = Arc::new(EffectJournal::new(1, 1_000).unwrap());
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::Reject {
            tx_hash: Byte32::zero(),
        })
        .unwrap();
    journal
        .enqueue(
            EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: Byte32::new([3; 32]),
            })])
            .unwrap(),
        )
        .await
        .unwrap();
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&journal),
        endpoints(relay_tx),
    ));

    tokio::time::sleep(RELAY_RETRY_TIMEOUT + Duration::from_millis(50)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), journal.wait_idle())
            .await
            .is_err(),
        "the coalesced reset remains authoritative while its sink is full"
    );
    assert!(matches!(
        relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == Byte32::zero()
    ));
    let reconciled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reconciliation must publish after capacity returns");
    assert!(matches!(reconciled, TxVerificationResult::GenerationReset));
    tokio::time::timeout(Duration::from_secs(1), journal.wait_idle())
        .await
        .expect("successful reconciliation releases the bounded journal");
    journal.close();
    publisher.await.unwrap();
}

#[test]
fn oversized_batch_never_executes_total_apply() {
    let journal = EffectJournal::new(1, 127).unwrap();
    let applied = AtomicUsize::new(0);
    let batch = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: Byte32::zero(),
    })])
    .unwrap();
    assert!(matches!(
        journal.try_apply(Some(batch), EffectClass::Trusted, || {
            applied.fetch_add(1, Ordering::SeqCst);
        }),
        Err(EffectJournalError::BatchTooLarge { .. })
    ));
    assert_eq!(applied.load(Ordering::SeqCst), 0);
}

#[test]
fn bounded_apply_charges_actual_not_conservative_ceiling() {
    let journal = EffectJournal::new(1, 1_000).unwrap();
    journal
        .try_apply_bounded(1_000, EffectClass::Trusted, || {
            (
                (),
                EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: Byte32::zero(),
                })]),
            )
        })
        .unwrap();
    assert_eq!(journal.usage().bytes, EFFECT_ENVELOPE_BYTES);
}

#[test]
fn post_apply_bound_violation_uses_reset_without_overcharging_fifo() {
    let journal = EffectJournal::new(1, 1_000).unwrap();
    let applied = AtomicUsize::new(0);
    journal
        .try_apply_bounded(1, EffectClass::Trusted, || {
            applied.fetch_add(1, Ordering::SeqCst);
            ((), Some(reject_batch(1)))
        })
        .unwrap();

    assert_eq!(applied.load(Ordering::SeqCst), 1, "Apply remains total");
    assert_eq!(journal.usage(), EffectUsage::default());
    let state = journal.state.lock().unwrap();
    assert!(state.queued.is_empty());
    assert!(state.latest_generation_reset.is_some());
}
