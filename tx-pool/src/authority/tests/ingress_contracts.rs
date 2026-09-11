use super::{
    super::{
        ingress,
        model::{Error, Phase, Source, Status},
        notice::Class,
        store::{Plan, ReadSet},
    },
    common::*,
};
use crate::error::Reject;
use ckb_types::{packed::OutPoint, prelude::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn malformed() -> Reject {
    Reject::Malformed("remote declared cycles".into(), "test".into())
}

#[test]
fn malformed_peer_revocation_is_atomic_and_preserves_other_peers_and_accepted_transactions() {
    let store = store();
    let source = ingress::remote_source(81.into(), 1).unwrap();
    let culprit = entry(&store, tx(8100), source);
    let sibling = entry(&store, tx(8101), source);
    let other = entry(
        &store,
        tx(8102),
        ingress::remote_source(82.into(), 1).unwrap(),
    );
    for owner in [&culprit, &sibling, &other] {
        insert(&store, Arc::clone(owner));
    }
    let accepted = accept(&store, output_tx(8103), 1, 1, Status::Pending);
    let plan = ingress::rejection(
        &store,
        store.snapshot().0,
        Some(Arc::clone(&culprit)),
        &culprit.hash(),
        source,
        malformed(),
        ReadSet::default(),
    )
    .unwrap();
    store.apply(plan).unwrap();
    assert!(store.peer_banned(81.into()));
    assert!(!store.peer_banned(82.into()));
    assert!(store.point(&culprit.hash()).1.is_none());
    assert!(store.point(&sibling.hash()).1.is_none());
    assert!(store.point(&other.hash()).1.is_some());
    assert!(store.point(&accepted).1.unwrap().accepted().is_some());
}

#[test]
fn old_worker_cannot_ban_a_peer_after_its_culprit_has_been_promoted() {
    let store = store();
    let source = ingress::remote_source(83.into(), 1).unwrap();
    let culprit = entry(&store, tx(8104), source);
    insert(&store, Arc::clone(&culprit));
    let stale = ingress::rejection(
        &store,
        store.snapshot().0,
        Some(Arc::clone(&culprit)),
        &culprit.hash(),
        source,
        malformed(),
        ReadSet::default(),
    )
    .unwrap();
    let promoted = Arc::new(super::super::model::Entry {
        source: Source::Recovery,
        ..culprit.as_ref().clone()
    });
    let mut promotion = Plan::new(store.snapshot().0, Class::Trusted);
    promotion
        .edit(Some(Arc::clone(&culprit)), Some(promoted))
        .unwrap();
    store.apply(promotion).unwrap();
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert!(!store.peer_banned(83.into()));
    assert!(store.point(&culprit.hash()).1.is_some());
}

#[test]
fn late_peer_admission_invalidates_an_older_complete_revocation_plan() {
    let store = store();
    let source = ingress::remote_source(84.into(), 1).unwrap();
    let culprit = entry(&store, tx(8105), source);
    insert(&store, Arc::clone(&culprit));
    let stale = ingress::rejection(
        &store,
        store.snapshot().0,
        Some(Arc::clone(&culprit)),
        &culprit.hash(),
        source,
        malformed(),
        ReadSet::default(),
    )
    .unwrap();
    let late = entry(&store, tx(8106), source);
    insert(&store, Arc::clone(&late));
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert!(!store.peer_banned(84.into()));
    assert!(store.point(&late.hash()).1.is_some());
}

#[test]
fn banned_remote_input_only_publishes_release_and_cannot_reenter() {
    let store = store();
    let mut ban = Plan::new(store.snapshot().0, Class::Remote);
    ban.ban = Some((85.into(), Instant::now() + Duration::from_secs(60)));
    store.apply(ban).unwrap();
    let transaction = funded_tx(OutPoint::new(tx(8199).hash(), 0), 20_000_000_000);
    let plan = ingress::prepare(
        &store,
        Arc::new(transaction.clone()),
        ingress::remote_source(85.into(), 1).unwrap(),
    )
    .unwrap();
    assert!(plan.edits.is_empty());
    assert_eq!(plan.effects.len(), 1);
    store.apply(plan).unwrap();
    assert!(store.point(&transaction.hash()).1.is_none());
    assert!(store.peer_banned(85.into()));
}

#[test]
fn newly_banned_peer_invalidates_a_prepared_remote_admission() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8198).hash(), 0), 20_000_000_000);
    let plan = ingress::prepare(
        &store,
        Arc::new(transaction.clone()),
        ingress::remote_source(86.into(), 1).unwrap(),
    )
    .unwrap();
    let mut ban = Plan::new(store.snapshot().0, Class::Remote);
    ban.ban = Some((86.into(), Instant::now() + Duration::from_secs(60)));
    store.apply(ban).unwrap();
    assert!(matches!(store.apply(plan), Err(Error::Stale)));
    assert!(store.point(&transaction.hash()).1.is_none());
}

#[test]
fn remote_duplicate_preserves_existing_payload_source_deadline_and_queue_owner() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8197).hash(), 0), 20_000_000_000);
    let source = ingress::remote_source(87.into(), 1).unwrap();
    let plan = ingress::prepare(&store, Arc::new(transaction.clone()), source).unwrap();
    store.apply(plan).unwrap();
    let before = store.point(&transaction.hash()).1.unwrap();
    let changed = transaction
        .as_advanced_builder()
        .witness(ckb_types::bytes::Bytes::from_static(b"different").pack())
        .build();
    store
        .apply(
            ingress::prepare(
                &store,
                Arc::new(changed),
                ingress::remote_source(88.into(), 999).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let after = store.point(&before.hash()).1.unwrap();
    assert!(Arc::ptr_eq(&before, &after));
    assert!(matches!(after.phase, Phase::Resolve));
    assert_eq!(after.source, source);
}

#[test]
fn declared_cycle_and_non_contextual_gates_reject_before_retaining_any_source() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8196).hash(), 0), 20_000_000_000);
    let source = ingress::remote_source(
        89.into(),
        store.snapshot().1.consensus().max_block_cycles() + 1,
    )
    .unwrap();
    let plan = ingress::prepare(&store, Arc::new(transaction.clone()), source).unwrap();
    assert!(plan.edits.is_empty());
    assert!(plan.ban.is_some());
    assert!(plan.effects.iter().any(|effect| matches!(
        effect.relay,
        Some(crate::service::TxVerificationResult::GenerationReset)
    )));
    store.apply(plan).unwrap();
    assert!(store.point(&transaction.hash()).1.is_none());
    assert!(store.peer_banned(89.into()));
    for source in [
        Source::Local,
        Source::Recovery,
        Source::Proposal { remote: None },
        ingress::remote_source(90.into(), 1).unwrap(),
    ] {
        let invalid = tx(8195);
        let plan = ingress::prepare(&store, Arc::new(invalid.clone()), source).unwrap();
        assert!(plan.edits.is_empty());
        assert!(!plan.effects.is_empty());
        store.apply(plan).unwrap();
        assert!(store.point(&invalid.hash()).1.is_none());
    }
    assert!(!store.is_faulted());
}

#[test]
fn repeated_proposal_and_recovery_witness_variants_preserve_the_same_owner() {
    for source in [Source::Recovery, Source::Proposal { remote: None }] {
        let store = store();
        let transaction = funded_tx(OutPoint::new(tx(8194).hash(), 0), 20_000_000_000);
        store
            .apply(ingress::prepare(&store, Arc::new(transaction.clone()), source).unwrap())
            .unwrap();
        let before = store.point(&transaction.hash()).1.unwrap();
        let changed = if source == Source::Recovery {
            transaction
                .as_advanced_builder()
                .witness(ckb_types::bytes::Bytes::from_static(b"alternate").pack())
                .build()
        } else {
            transaction
        };
        let plan =
            ingress::prepare(&store, Arc::new(changed), Source::Proposal { remote: None }).unwrap();
        assert!(plan.edits.is_empty());
        assert!(plan.effects.is_empty());
        store.apply(plan).unwrap();
        assert!(Arc::ptr_eq(
            &store.point(&before.hash()).1.unwrap(),
            &before
        ));
    }
}

#[test]
fn accepted_remote_duplicate_acknowledges_the_new_peer_without_changing_ownership() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8193).hash(), 0), 20_000_000_000);
    let hash = accept(&store, transaction.clone(), 1000, 17, Status::Pending);
    let before = store.point(&hash).1.unwrap();
    let plan = ingress::prepare(
        &store,
        Arc::new(transaction),
        ingress::remote_source(91.into(), 17).unwrap(),
    )
    .unwrap();
    assert!(plan.edits.is_empty());
    assert!(
        matches!(plan.effects[0].relay, Some(crate::service::TxVerificationResult::Ok {
        original_peer: Some(peer), ref tx_hash }) if peer == 91.into() && tx_hash == &hash)
    );
    store.apply(plan).unwrap();
    assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &before));
    assert!(!store.peer_banned(91.into()));
}

#[test]
fn active_peer_ban_survives_both_full_and_pipeline_clear() {
    for pipeline in [false, true] {
        let store = store();
        let mut ban = Plan::new(store.snapshot().0, Class::Remote);
        ban.ban = Some((92.into(), Instant::now() + Duration::from_secs(60)));
        store.apply(ban).unwrap();
        store
            .apply(super::super::chain::clear(&store, None, pipeline).unwrap())
            .unwrap();
        assert!(store.peer_banned(92.into()));
        let transaction = funded_tx(OutPoint::new(tx(8192).hash(), 0), 20_000_000_000);
        let plan = ingress::prepare(
            &store,
            Arc::new(transaction),
            ingress::remote_source(92.into(), 1).unwrap(),
        )
        .unwrap();
        assert!(plan.edits.is_empty());
        assert_eq!(plan.effects.len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promotion_reuses_exact_resolution_and_rechecks_original_producers() {
    use super::super::{jobs, queue::WorkStage, store::Store};
    use ckb_script::{ChunkCommand, TxPoolVmExecutionMode};
    use ckb_verification::cache::init_cache;
    use tokio::sync::{RwLock, watch};

    for remove_producer in [false, true] {
        let store = Store::new(chain_snapshot(), &config()).unwrap();
        let parent = funded_parent(8250, 20_000_000_000);
        let parent_hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
        let transaction = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000);
        let source = ingress::remote_source(92.into(), 1).unwrap();
        store
            .apply(ingress::prepare(&store, Arc::new(transaction.clone()), source).unwrap())
            .unwrap();
        let before = store.point(&transaction.hash()).1.unwrap();
        let jobs::Resolution::Ready(resolved) = jobs::resolve(&store, &before, &config()).unwrap()
        else {
            panic!("funded transaction resolves");
        };
        let verifying = replace(&store, before, Phase::Verify(Arc::clone(&resolved)));
        let active = store.pop(WorkStage::Verify, false).unwrap().unwrap();
        assert!(active.current().unwrap());
        let obsolete_rejection = ingress::rejection(
            &store,
            store.snapshot().0,
            Some(Arc::clone(&verifying)),
            &verifying.hash(),
            source,
            malformed(),
            ReadSet::default(),
        )
        .unwrap();
        if remove_producer {
            let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
            removal.edit(store.point(&parent_hash).1, None).unwrap();
            store.apply(removal).unwrap();
        }
        store
            .apply(
                ingress::prepare(
                    &store,
                    Arc::new(transaction),
                    Source::Proposal { remote: None },
                )
                .unwrap(),
            )
            .unwrap();
        let promoted = store.point(&verifying.hash()).1.unwrap();
        let Phase::Verify(reused) = &promoted.phase else {
            panic!("same-payload promotion keeps the resolved value");
        };
        assert!(Arc::ptr_eq(reused, &resolved));
        assert_eq!(promoted.arrival, verifying.arrival);
        assert_eq!(promoted.source.residency_peer(), source.residency_peer());
        assert_eq!(promoted.source.deadline(), source.deadline());
        assert_eq!(promoted.source.compute_peer(), None);
        assert_eq!(promoted.source.declared_cycles(), None);
        assert!(!active.current().unwrap());
        drop(active);
        assert!(matches!(store.apply(obsolete_rejection), Err(Error::Stale)));
        assert!(!store.peer_banned(92.into()));
        let mut next = store.pop(WorkStage::Verify, false).unwrap().unwrap();
        assert!(Arc::ptr_eq(&next.entry, &promoted));
        let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
        let result = jobs::verify(
            &store,
            &promoted,
            Arc::clone(reused),
            &config(),
            &RwLock::new(init_cache()),
            &mut commands,
            TxPoolVmExecutionMode::Inline,
        )
        .await;
        if remove_producer {
            assert!(matches!(result, Err(Error::Stale)));
        } else {
            // The old remote declaration was one cycle. Current proposal
            // policy is evaluated afresh even though the cells are reused.
            assert!(result.unwrap().cycles() > 1);
        }
        next.complete();
        drop(next);
        assert!(!store.is_faulted());
    }
}

#[test]
fn proposal_promotion_resolves_again_for_changed_witness_or_resolution_view() {
    use super::super::{jobs, store::Store};
    for change_witness in [false, true] {
        let store = Store::new(chain_snapshot(), &config()).unwrap();
        let parent = funded_parent(8251, 20_000_000_000);
        accept(&store, parent.clone(), 1, 1, Status::Pending);
        let transaction = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000);
        let before = entry(
            &store,
            transaction.clone(),
            ingress::remote_source(93.into(), 1).unwrap(),
        );
        let jobs::Resolution::Ready(mut resolved) =
            jobs::resolve(&store, &before, &config()).unwrap()
        else {
            panic!("funded transaction resolves");
        };
        let transaction = if change_witness {
            transaction
                .as_advanced_builder()
                .witness(ckb_types::bytes::Bytes::from_static(b"new witness").pack())
                .build()
        } else {
            // A stale queued resolution may never bypass its lifecycle check.
            Arc::make_mut(&mut resolved).view += 1;
            transaction
        };
        let verifying = before.with_phase(Phase::Verify(resolved));
        insert(&store, Arc::clone(&verifying));
        let promotion = ingress::prepare(
            &store,
            Arc::new(transaction.clone()),
            Source::Proposal { remote: None },
        )
        .unwrap();
        store.apply(promotion).unwrap();
        let promoted = store.point(&verifying.hash()).1.unwrap();
        assert!(matches!(promoted.phase, Phase::Resolve));
        assert_eq!(
            promoted.transaction.witness_hash(),
            transaction.witness_hash()
        );
        assert!(!Arc::ptr_eq(&promoted, &verifying));
        assert!(!store.is_faulted());
    }
}
