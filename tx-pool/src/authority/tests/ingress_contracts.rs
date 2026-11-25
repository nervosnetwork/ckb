use super::{
    super::{
        chain::{self, ClearScope},
        ingress,
        model::{Error, FullReason, Phase, Premises, Source, Status},
        notice::{Class, Effect},
        service::ComputePermit,
        store::Store,
    },
    common::*,
};
use crate::error::Reject;
use ckb_types::{packed::OutPoint, prelude::*};
use std::sync::Arc;

fn malformed() -> Reject {
    Reject::Malformed("remote declared cycles".into(), "test".into())
}

fn ban_peer(store: &Store, peer: usize) {
    let source = ingress::remote_source(peer.into(), 1).unwrap();
    ingress::commit_rejection(
        store,
        store.snapshot().0,
        &Premises::default(),
        None,
        &tx(90_000 + peer as u32).hash(),
        source,
        malformed(),
    )
    .unwrap();
}

#[test]
fn ingress_capacity_refusal_preserves_ownership_and_does_not_record_rejection() {
    for present in [false, true] {
        let store = store();
        let owner = entry(&store, tx(91_004), remote(93, 1));
        if present {
            insert(&store, Arc::clone(&owner));
        }
        let batch = ingress::commit_capacity_refusal(
            &store,
            &owner.hash(),
            owner.source,
            FullReason::Pipeline,
        )
        .unwrap()
        .unwrap();
        assert_eq!(batch.effects().len(), 1);
        assert!(batch.effects()[0].callback().is_none());
        assert!(matches!(
            batch.effects()[0].relay_result(),
            Some(crate::service::TxVerificationResult::Reject { tx_hash })
                if *tx_hash == owner.hash()
        ));
        assert_eq!(store.point(&owner.hash()).1.is_some(), present);
        if present {
            assert!(Arc::ptr_eq(&store.point(&owner.hash()).1.unwrap(), &owner));
        }
        assert!(store.outbox.pending_reject(&owner.hash()).is_none());
        assert!(!store.peer_banned(93.into()));
    }
}

#[test]
fn rejected_admission_preserves_incumbents_and_records_only_candidate_rejection() {
    let store = store();
    let shared = OutPoint::new(tx(91_000).hash(), 0);
    let victim = accept(
        &store,
        spend(91_001, std::slice::from_ref(&shared), &[]),
        10_000,
        100,
        Status::Pending,
    );
    let old = store.point(&victim).1.unwrap();
    let candidate = entry(
        &store,
        spend(91_002, std::slice::from_ref(&shared), &[]),
        Source::Local,
    );
    let view = store.snapshot().0;
    let rbf_config = ckb_app_config::TxPoolConfig {
        min_rbf_rate: ckb_types::core::FeeRate::from_u64(1_000),
        ..config()
    };
    let (batch, reject) =
        commit_admission(&store, &candidate, 100, 100, Status::Pending, &rbf_config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    let batch = batch.unwrap();
    assert_eq!(batch.effects().len(), 1);
    assert!(batch.effects()[0].callback().is_none());
    assert!(store.outbox.pending_reject(&candidate.hash()).is_some());
    assert!(Arc::ptr_eq(&old, &store.point(&victim).1.unwrap()));
    assert!(store.point(&candidate.hash()).1.is_none());
    assert!(store.outbox.pending_reject(&victim).is_none());
    assert_eq!(store.snapshot().0, view);
}

#[test]
fn rejected_preview_validates_premises_without_consuming_notice_capacity_or_retiring_its_owner() {
    let store = store();
    let shared = OutPoint::new(tx(91_003).hash(), 0);
    accept(
        &store,
        spend(91_004, std::slice::from_ref(&shared), &[]),
        10_000,
        100,
        Status::Pending,
    );
    let owner = entry(&store, spend(91_005, &[shared], &[]), remote(92, 1));
    insert(&store, Arc::clone(&owner));
    let held: Vec<_> = (0..crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES)
        .map(|_| {
            store
                .outbox
                .commit(vec![Effect::reset()], Class::Remote)
                .unwrap()
                .unwrap()
        })
        .collect();
    assert!(matches!(
        store.outbox.commit(vec![Effect::reset()], Class::Remote),
        Err(Error::Full(FullReason::NoticeOutbox))
    ));
    let rbf_config = ckb_app_config::TxPoolConfig {
        min_rbf_rate: ckb_types::core::FeeRate::from_u64(1_000),
        ..config()
    };
    let reject = preview_admission(&store, &owner, 100, 100, Status::Pending, &rbf_config).unwrap();
    assert!(matches!(reject, Some(Reject::RBFRejected(_))));
    assert!(Arc::ptr_eq(&owner, &store.point(&owner.hash()).1.unwrap()));
    assert!(store.outbox.pending_reject(&owner.hash()).is_none());

    // A stale observed premise rejects even in preview mode.
    let verified = verification_fixture(&store, &owner, 100, 100, Status::Pending);
    replace(&store, Arc::clone(&owner), Phase::Resolve);
    assert!(matches!(
        super::super::membership::commit_admission(
            &store,
            &owner,
            Some(Arc::clone(&owner)),
            &verified,
            &config(),
            super::super::membership::AdmissionOptions {
                retain_history: false,
                dry_run: true,
                inline_publish: false,
                silence_effects: false,
                forbid_displacement: false,
            },
        ),
        Err(Error::Stale)
    ));
    drop(held);
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
    let accepted_remote = entry(&store, output_tx(8104), source);
    let (_, reject) =
        commit_admission(&store, &accepted_remote, 1, 1, Status::Pending, &config()).unwrap();
    assert!(reject.is_none());
    ingress::commit_rejection(
        &store,
        store.snapshot().0,
        &Premises::default(),
        Some(Arc::clone(&culprit)),
        &culprit.hash(),
        source,
        malformed(),
    )
    .unwrap();
    assert!(store.peer_banned(81.into()));
    assert!(!store.peer_banned(82.into()));
    assert!(store.point(&culprit.hash()).1.is_none());
    assert!(store.point(&sibling.hash()).1.is_none());
    assert!(store.point(&other.hash()).1.is_some());
    assert!(store.point(&accepted).1.unwrap().accepted().is_some());
    let accepted_remote = store.point(&accepted_remote.hash()).1.unwrap();
    assert!(accepted_remote.accepted().is_some());
    assert_eq!(accepted_remote.source, source);
}

#[test]
fn old_worker_cannot_ban_a_peer_after_its_culprit_has_been_promoted() {
    let store = store();
    let source = ingress::remote_source(83.into(), 1).unwrap();
    let culprit = entry(&store, tx(8104), source);
    insert(&store, Arc::clone(&culprit));
    let view = store.snapshot().0;
    let promoted = culprit.with_source(Source::Recovery);
    commit_pairs(&store, vec![(Some(Arc::clone(&culprit)), Some(promoted))]).unwrap();
    assert!(matches!(
        ingress::commit_rejection(
            &store,
            view,
            &Premises::default(),
            Some(Arc::clone(&culprit)),
            &culprit.hash(),
            source,
            malformed(),
        ),
        Err(Error::Stale)
    ));
    assert!(!store.peer_banned(83.into()));
    assert!(store.point(&culprit.hash()).1.is_some());
}

#[test]
fn late_peer_admission_is_revoked_atomically_with_the_culprit() {
    let store = store();
    let source = ingress::remote_source(84.into(), 1).unwrap();
    let culprit = entry(&store, tx(8105), source);
    insert(&store, Arc::clone(&culprit));
    let view = store.snapshot().0;
    let late = entry(&store, tx(8106), source);
    insert(&store, Arc::clone(&late));
    // Under single-writer commit, peer_members is read directly under the write
    // lock, so banning the peer revokes both culprit and late in one atomic cut.
    ingress::commit_rejection(
        &store,
        view,
        &Premises::default(),
        Some(Arc::clone(&culprit)),
        &culprit.hash(),
        source,
        malformed(),
    )
    .unwrap();
    assert!(store.peer_banned(84.into()));
    assert!(store.point(&culprit.hash()).1.is_none());
    assert!(store.point(&late.hash()).1.is_none());
}

#[test]
fn banned_remote_input_only_publishes_release_and_cannot_reenter() {
    let store = store();
    ban_peer(&store, 85);
    let transaction = funded_tx(OutPoint::new(tx(8199).hash(), 0), 20_000_000_000);
    let batch = ingress::commit_ingress(
        &store,
        Arc::new(transaction.clone()),
        ingress::remote_source(85.into(), 1).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(batch.effects().len(), 1);
    assert!(store.point(&transaction.hash()).1.is_none());
    assert!(store.peer_banned(85.into()));
}

#[test]
fn newly_banned_peer_blocks_subsequent_remote_admission() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8198).hash(), 0), 20_000_000_000);
    ban_peer(&store, 86);
    let batch = ingress::commit_ingress(
        &store,
        Arc::new(transaction.clone()),
        ingress::remote_source(86.into(), 1).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(batch.effects().len(), 1);
    assert!(store.point(&transaction.hash()).1.is_none());
}

#[test]
fn remote_duplicate_preserves_existing_payload_source_deadline_and_queue_owner() {
    let store = store();
    let transaction = funded_tx(OutPoint::new(tx(8197).hash(), 0), 20_000_000_000);
    let source = ingress::remote_source(87.into(), 1).unwrap();
    ingress::commit_ingress(&store, Arc::new(transaction.clone()), source).unwrap();
    let before = store.point(&transaction.hash()).1.unwrap();
    let changed = transaction
        .as_advanced_builder()
        .witness(ckb_types::bytes::Bytes::from_static(b"different").pack())
        .build();
    ingress::commit_ingress(
        &store,
        Arc::new(changed),
        ingress::remote_source(88.into(), 999).unwrap(),
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
    let batch = ingress::commit_ingress(&store, Arc::new(transaction.clone()), source)
        .unwrap()
        .unwrap();
    assert!(batch.effects().iter().any(|effect| matches!(
        effect.relay_result(),
        Some(crate::service::TxVerificationResult::GenerationReset)
    )));
    assert!(store.point(&transaction.hash()).1.is_none());
    assert!(store.peer_banned(89.into()));
    for source in [
        Source::Local,
        Source::Recovery,
        Source::Proposal { remote: None },
        ingress::remote_source(90.into(), 1).unwrap(),
    ] {
        let invalid = tx(8195);
        let batch = ingress::commit_ingress(&store, Arc::new(invalid.clone()), source)
            .unwrap()
            .unwrap();
        assert!(!batch.effects().is_empty());
        assert!(store.point(&invalid.hash()).1.is_none());
    }
    assert!(!store.is_faulted());
}

#[test]
fn repeated_proposal_and_recovery_witness_variants_preserve_the_same_owner() {
    for source in [Source::Recovery, Source::Proposal { remote: None }] {
        let store = store();
        let transaction = funded_tx(OutPoint::new(tx(8194).hash(), 0), 20_000_000_000);
        ingress::commit_ingress(&store, Arc::new(transaction.clone()), source).unwrap();
        let before = store.point(&transaction.hash()).1.unwrap();
        let changed = if source == Source::Recovery {
            transaction
                .as_advanced_builder()
                .witness(ckb_types::bytes::Bytes::from_static(b"alternate").pack())
                .build()
        } else {
            transaction
        };
        let batch =
            ingress::commit_ingress(&store, Arc::new(changed), Source::Proposal { remote: None })
                .unwrap();
        assert!(batch.is_none());
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
    let batch = ingress::commit_ingress(
        &store,
        Arc::new(transaction),
        ingress::remote_source(91.into(), 17).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(matches!(
        batch.effects()[0].relay_result(),
        Some(crate::service::TxVerificationResult::Ok {
            original_peer: Some(peer),
            tx_hash,
        }) if *peer == 91.into() && tx_hash == &hash
    ));
    assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &before));
    assert!(!store.peer_banned(91.into()));
}

#[test]
fn active_peer_ban_survives_both_full_and_pipeline_clear() {
    for scope in [ClearScope::All, ClearScope::Unaccepted] {
        let store = store();
        ban_peer(&store, 92);
        chain::commit_clear(&store, None, scope).unwrap();
        assert!(store.peer_banned(92.into()));
        let transaction = funded_tx(OutPoint::new(tx(8192).hash(), 0), 20_000_000_000);
        let batch = ingress::commit_ingress(
            &store,
            Arc::new(transaction.clone()),
            ingress::remote_source(92.into(), 1).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(batch.effects().len(), 1);
        assert!(store.point(&transaction.hash()).1.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promotion_reuses_exact_resolution_and_rechecks_original_producers() {
    use super::super::{
        jobs,
        queue::{WorkSelection, WorkStage},
    };
    use ckb_script::ChunkCommand;
    use ckb_verification::cache::init_cache;
    use tokio::sync::{RwLock, watch};

    for remove_producer in [false, true] {
        let store = Store::new(chain_snapshot(), &config()).unwrap();
        let parent = funded_parent(8250, 20_000_000_000);
        let parent_hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
        let transaction = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000);
        let source = ingress::remote_source(92.into(), 1).unwrap();
        ingress::commit_ingress(&store, Arc::new(transaction.clone()), source).unwrap();
        let before = store.point(&transaction.hash()).1.unwrap();
        let jobs::Resolution::Ready(resolved) = jobs::resolve(&store, &before, &config()).unwrap()
        else {
            panic!("funded transaction resolves");
        };
        let verifying = replace(&store, before, Phase::Verify(Arc::clone(&resolved)));
        let active = store
            .pop(WorkStage::Verify, WorkSelection::Any)
            .unwrap()
            .unwrap();
        assert!(active.current());
        let obsolete_view = store.snapshot().0;
        if remove_producer {
            delete(&store, store.point(&parent_hash).1.unwrap()).unwrap();
        }
        ingress::commit_ingress(
            &store,
            Arc::new(transaction),
            Source::Proposal { remote: None },
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
        assert!(!active.current());
        drop(active);
        assert!(matches!(
            ingress::commit_rejection(
                &store,
                obsolete_view,
                &Premises::default(),
                Some(Arc::clone(&verifying)),
                &verifying.hash(),
                source,
                malformed(),
            ),
            Err(Error::Stale)
        ));
        assert!(!store.peer_banned(92.into()));
        let mut next = store
            .pop(WorkStage::Verify, WorkSelection::Any)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&next.entry, &promoted));
        let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
        let result = jobs::verify(
            &store,
            &promoted,
            Arc::clone(reused),
            &config(),
            &RwLock::new(init_cache()),
            &mut commands,
            &ComputePermit::for_test(),
        )
        .await;
        if remove_producer {
            assert!(matches!(result, Err(Error::Stale)));
        } else {
            // The old remote declaration was one cycle. Current proposal
            // policy is evaluated afresh even though the cells are reused.
            assert!(result.unwrap().cycles() > 1);
        }
        next.mark_handled();
        drop(next);
        assert!(!store.is_faulted());
    }
}

#[test]
fn proposal_promotion_resolves_again_for_changed_witness_or_resolution_view() {
    use super::super::jobs;
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
        ingress::commit_ingress(
            &store,
            Arc::new(transaction.clone()),
            Source::Proposal { remote: None },
        )
        .unwrap();
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
