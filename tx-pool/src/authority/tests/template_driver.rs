use super::*;
use crate::authority::{
    chain, membership,
    model::{FullReason, Phase, Status},
    tests::common::*,
};
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_snapshot::Snapshot;
use ckb_types::h256;
use std::{collections::BTreeSet, sync::Arc, time::Duration};

fn template_config() -> BlockAssemblerConfig {
    BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
        update_interval_millis: 60_000,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
        notify_auth_token: None,
    }
}

fn template_snapshot() -> Arc<Snapshot> {
    template_snapshot_with_child(None)
}

fn fixture() -> Arc<Driver> {
    let snapshot = template_snapshot();
    let store = Store::new(Arc::clone(&snapshot), &config()).unwrap();
    let assembler = BlockAssembler::new(template_config(), snapshot).unwrap();
    Driver::new(store, assembler, config().max_ancestors_count)
}
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("template event completes")
}
fn selected(driver: &Driver) -> Arc<CurrentTemplate> {
    Arc::clone(&driver.assembler.current.read())
}

#[test]
fn mandatory_template_accepts_exact_byte_limit_and_rejects_one_byte_less() {
    let initial = BlockAssembler::new(template_config(), template_snapshot()).unwrap();
    let required = initial.current.read().size.total;
    assert!(required > 0);
    let snapshot = |limit| {
        template_snapshot_with_consensus(
            None,
            Arc::new(ConsensusBuilder::default().max_block_bytes(limit).build()),
        )
    };
    let exact = BlockAssembler::new(template_config(), snapshot(required as u64)).unwrap();
    let current = exact.current.read();
    assert_eq!(current.size.total, required);
    assert_eq!(current.template.bytes_limit, required as u64);
    assert!(current.template.transactions.is_empty());
    assert!(current.template.proposals.is_empty());
    assert!(current.template.uncles.is_empty());
    drop(current);
    let error = BlockAssembler::new(template_config(), snapshot((required - 1) as u64))
        .err()
        .expect("mandatory content cannot exceed the consensus byte limit");
    assert!(matches!(
        error.downcast_ref::<BlockAssemblerError>(),
        Some(BlockAssemblerError::Overflow)
    ));
}

#[test]
fn template_build_reproposes_a_recovered_gap_then_packs_it_after_proposal() {
    let driver = fixture();
    let pending = tx(7100);
    let gap = tx(7101);
    accept(&driver.store, pending.clone(), 1, 1, Status::Pending);
    accept(&driver.store, gap.clone(), 1, 1, Status::Gap);
    driver.rebuild().unwrap();
    let output = selected(&driver);
    assert_eq!(
        output.template.parent_hash,
        driver.store.snapshot().1.tip_hash()
    );
    assert_eq!(
        output
            .template
            .proposals
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        [pending.proposal_short_id()].into()
    );
    let source = output.source.as_ref().unwrap();
    driver
        .store
        .read_selected(source.view, &source.reads, || ())
        .unwrap();
    let owner = driver.store.point(&gap.hash()).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.forced_status = Some(Status::Pending);
    let pending_owner = replace(&driver.store, owner, Phase::Accepted(value));
    driver.rebuild().unwrap();
    assert_eq!(selected(&driver).template.proposals.len(), 2);
    let mut value = pending_owner.accepted().unwrap().clone();
    value.forced_status = Some(Status::Proposed);
    replace(&driver.store, pending_owner, Phase::Accepted(value));
    driver.rebuild().unwrap();
    let output = selected(&driver);
    assert_eq!(output.template.proposals, vec![pending.proposal_short_id()]);
    assert_eq!(output.template.transactions.len(), 1);
    assert_eq!(
        output.template.transactions[0].transaction().hash(),
        gap.hash()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_acceptance_does_not_invalidate_current_selected_output() {
    let driver = fixture();
    accept(&driver.store, tx(7102), 1, 1, Status::Pending);
    driver.rebuild().unwrap();
    let original = within(driver.read()).await.unwrap();
    accept(&driver.store, tx(7103), 1, 1, Status::Pending);
    let unchanged = within(driver.read()).await.unwrap();
    assert_eq!(unchanged.work_id, original.work_id);
    assert_eq!(unchanged.proposals, original.proposals);
    driver.rebuild().unwrap();
    assert_eq!(within(driver.read()).await.unwrap().proposals.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_selected_proposal_requires_a_fresh_publication_before_read() {
    let driver = fixture();
    let hash = accept(&driver.store, tx(7104), 1, 1, Status::Pending);
    driver.rebuild().unwrap();
    let old = selected(&driver);
    let entry = driver.store.point(&hash).1.unwrap();
    driver
        .store
        .apply(membership::removal(&driver.store, &entry, &config(), None).unwrap())
        .unwrap();
    let source = old.source.as_ref().unwrap();
    assert!(matches!(
        driver
            .store
            .read_selected(source.view, &source.reads, || ()),
        Err(Error::Stale)
    ));
    let task = tokio::spawn(Arc::clone(&driver).run());
    let output = within(driver.read()).await.unwrap();
    assert!(output.proposals.is_empty());
    driver.store.stop();
    within(task).await.unwrap().unwrap();
}

#[test]
fn prepared_template_cannot_rebind_a_reentered_selected_owner() {
    let driver = fixture();
    let transaction = tx(7105);
    let hash = accept(&driver.store, transaction.clone(), 1, 1, Status::Pending);
    let (prepared, _, _) = driver.prepare().unwrap();
    let old = driver.store.point(&hash).1.unwrap();
    driver
        .store
        .apply(membership::removal(&driver.store, &old, &config(), None).unwrap())
        .unwrap();
    accept(&driver.store, transaction, 1, 1, Status::Pending);
    let source = prepared.source.as_ref().unwrap();
    assert!(matches!(
        driver
            .store
            .read_selected(source.view, &source.reads, || ()),
        Err(Error::Stale)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_tip_clear_still_requires_a_new_template_source_and_joins() {
    let driver = fixture();
    accept(&driver.store, tx(7106), 1, 1, Status::Pending);
    driver.rebuild().unwrap();
    let old = selected(&driver);
    driver
        .store
        .apply(chain::clear(&driver.store, None, false).unwrap())
        .unwrap();
    let source = old.source.as_ref().unwrap();
    assert!(matches!(
        driver
            .store
            .read_selected(source.view, &source.reads, || ()),
        Err(Error::Stale)
    ));
    let task = tokio::spawn(Arc::clone(&driver).run());
    let notify = tokio::spawn(Arc::clone(&driver).notify());
    assert!(within(driver.read()).await.unwrap().proposals.is_empty());
    driver.store.stop();
    within(task).await.unwrap().unwrap();
    within(notify).await.unwrap().unwrap();
    assert!(matches!(driver.read().await, Err(Error::Closed)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_new_source_build_does_not_return_the_previous_tip_template() {
    let driver = fixture();
    driver.rebuild().unwrap();
    let old = selected(&driver);
    let unavailable = crate::test_support::genesis_snapshot();
    driver
        .store
        .apply(chain::clear(&driver.store, Some(unavailable), false).unwrap())
        .unwrap();
    let task = tokio::spawn(Arc::clone(&driver).run());
    assert!(matches!(
        within(driver.read()).await,
        Err(Error::Full(FullReason::Other("template build")))
    ));
    assert!(Arc::ptr_eq(&selected(&driver), &old));
    assert!(!driver.store.is_faulted());
    driver.store.stop();
    within(task).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn template_refresh_wait_has_one_deadline_and_keeps_valid_cache_available() {
    let driver = fixture();
    driver.rebuild().unwrap();
    let old = selected(&driver);
    driver
        .store
        .apply(chain::clear(&driver.store, None, false).unwrap())
        .unwrap();
    let retained = Arc::strong_count(&old);
    let read = driver.read();
    tokio::pin!(read);
    assert!(futures_util::poll!(&mut read).is_pending());
    assert_eq!(Arc::strong_count(&old), retained);
    tokio::time::advance(TEMPLATE_REFRESH_WAIT / 2).await;
    // A completed but stale shared build may wake readers. That wake must not
    // renew a request's original deadline under repeated source changes.
    driver.updated.notify_waiters();
    assert!(futures_util::poll!(&mut read).is_pending());
    tokio::time::advance(TEMPLATE_REFRESH_WAIT / 2).await;
    driver.updated.notify_waiters();
    assert!(matches!(
        read.await,
        Err(Error::Full(FullReason::Other("template refresh timeout")))
    ));
    assert!(Arc::ptr_eq(&selected(&driver), &old));
    assert!(!driver.store.is_faulted());
    driver.rebuild().unwrap();
    assert!(driver.read().await.unwrap().proposals.is_empty());
    driver.store.stop();
    assert!(matches!(driver.read().await, Err(Error::Closed)));
}
