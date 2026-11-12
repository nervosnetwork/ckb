//! Exported accepted-status gauges follow committed owners and their paired view.
use super::common::*;
use crate::authority::{
    chain::{self, ClearScope},
    membership,
    model::{Error, Phase, RecoveryTriggers, Source, Status},
    store::Store,
};
use crate::service::ChainReorgArgs;
use ckb_metrics::Encoder;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    core::{BlockBuilder, BlockView},
    packed::ProposalShortId,
};
use std::{collections::BTreeMap, sync::Arc};

/// Exercise the same registry and text encoder as the HTTP exporter, including
/// the metric name and its complete label set rather than only gauge handles.
pub(in crate::authority) fn exported_entries() -> [i64; 3] {
    ckb_metrics::handle().expect("metrics enabled for this isolated test");
    let mut bytes = Vec::new();
    ckb_metrics::TextEncoder::new()
        .encode(&ckb_metrics::gather(), &mut bytes)
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let mut series = BTreeMap::new();
    for line in text
        .lines()
        .filter(|line| line.starts_with("ckb_tx_pool_entry{"))
    {
        let (name, value) = line.split_once(' ').unwrap();
        assert!(series.insert(name, value.parse::<i64>().unwrap()).is_none());
    }
    let names = [
        "ckb_tx_pool_entry{type=\"pending\"}",
        "ckb_tx_pool_entry{type=\"gap\"}",
        "ckb_tx_pool_entry{type=\"proposed\"}",
    ];
    assert_eq!(series.len(), names.len(), "exactly three exported statuses");
    names.map(|name| *series.get(name).expect("exported status series"))
}

fn assert_entries(store: &Store, expected: [i64; 3]) {
    store.publish_entry_metrics();
    assert_eq!(exported_entries(), expected);
}

#[test]
fn entry_metrics_export_admission_removal_and_clear_without_pipeline_owners() {
    ckb_metrics::METRICS_SERVICE_ENABLED.set(true).unwrap();
    let store = store();
    assert_entries(&store, [0, 0, 0]);

    let resolving = entry(&store, tx(31_000), Source::Local);
    let verifying = queued(&store, 31_001, Source::Local, 1);
    let waiting =
        entry(&store, tx(31_002), Source::Local).with_phase(Phase::Waiting(Default::default()));
    let history = entry(&store, tx(31_003), Source::Recovery).with_phase(Phase::Replaced(
        RecoveryTriggers::RetryInputs(Default::default()),
    ));
    for owner in [resolving, verifying, waiting, history] {
        insert(&store, owner);
        assert_entries(&store, [0, 0, 0]);
    }

    let statuses = [Status::Pending, Status::Gap, Status::Proposed];
    let mut hashes = Vec::new();
    let mut expected = [0; 3];
    for (index, status) in statuses.into_iter().enumerate() {
        hashes.push(accept(
            &store,
            output_tx(31_010 + index as u32),
            1,
            1,
            status,
        ));
        expected[index] = 1;
        assert_entries(&store, expected);
    }

    let first = store.point(&hashes[0]).1.unwrap();
    store
        .apply(delete(&store, Arc::clone(&first)).dry_run())
        .unwrap();
    assert_entries(&store, expected);
    let stale_removal = delete(&store, Arc::clone(&first));
    replace(&store, Arc::clone(&first), first.phase.clone());
    assert!(matches!(store.apply(stale_removal), Err(Error::Stale)));
    assert_entries(&store, expected);

    for (index, hash) in hashes.into_iter().enumerate() {
        let owner = store.point(&hash).1.unwrap();
        store
            .apply(membership::removal(&store, &owner, &config(), None).unwrap())
            .unwrap();
        expected[index] = 0;
        assert_entries(&store, expected);
    }

    for (index, status) in statuses.into_iter().enumerate() {
        accept(&store, output_tx(31_020 + index as u32), 1, 1, status);
    }
    store
        .apply(chain::clear(&store, None, ClearScope::Unaccepted).unwrap())
        .unwrap();
    assert_entries(&store, [1, 1, 1]);
    store
        .apply(chain::clear(&store, None, ClearScope::All).unwrap())
        .unwrap();
    assert_entries(&store, [0, 0, 0]);
}

fn snapshot(store: &Store, tip: &BlockView, proposals: ProposalView) -> Arc<Snapshot> {
    let base = store.snapshot().1;
    Arc::new(Snapshot::new(
        tip.header(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        MockStore::default().store().get_snapshot(),
        proposals,
        base.cloned_consensus(),
    ))
}

fn apply_chain(store: &Store, command: ChainReorgArgs) {
    let _pause = store.begin_chain().unwrap();
    store
        .apply(chain::reconcile(store, &command, &config()).unwrap())
        .unwrap();
}

#[test]
fn entry_metrics_follow_proposal_windows_reorgs_and_detached_recovery() {
    ckb_metrics::METRICS_SERVICE_ENABLED.set(true).unwrap();
    // A small canonical window uses the proposal merge; a large unrelated
    // window forces the bounded owner lookup fallback for the same transitions.
    for unrelated in [0, 4_096] {
        let store = store();
        let transaction = output_tx(31_100);
        let hash = accept(&store, transaction.clone(), 1, 1, Status::Pending);
        let owner = store.point(&hash).1.unwrap();
        let mut accepted = owner.accepted().unwrap().clone();
        accepted.forced_status = None;
        replace(&store, owner, Phase::Accepted(accepted));
        assert_entries(&store, [1, 0, 0]);

        let mut previous: Option<BlockView> = None;
        for (step, (status, overlap, reorg)) in [
            (Status::Gap, false, false),
            (Status::Proposed, true, false),
            (Status::Proposed, false, false),
            (Status::Gap, false, true),
            (Status::Pending, false, true),
            (Status::Gap, false, false),
            (Status::Pending, false, true),
        ]
        .into_iter()
        .enumerate()
        {
            let base = store.snapshot().1;
            let height = base.tip_number() + u64::from(!reorg);
            let parent = if reorg {
                previous.as_ref().unwrap().parent_hash()
            } else {
                base.tip_hash()
            };
            let block = BlockBuilder::default()
                .number(height)
                .parent_hash(parent)
                .nonce(step as u128)
                .epoch(base.epoch_ext().number_with_fraction(height))
                .transaction(tx(0))
                .build();
            let mut gap: Vec<_> = (0..unrelated)
                .map(|index: u32| {
                    let mut bytes = [0; 10];
                    bytes[..4].copy_from_slice(&index.to_be_bytes());
                    ProposalShortId::new(bytes)
                })
                .filter(|id| *id != transaction.proposal_short_id())
                .collect();
            if status == Status::Gap || overlap {
                gap.push(transaction.proposal_short_id());
            }
            let proposed = (status == Status::Proposed)
                .then(|| transaction.proposal_short_id())
                .into_iter();
            let next = snapshot(&store, &block, ProposalView::new(gap, proposed));
            let detached = if reorg {
                [previous.take().unwrap()].into()
            } else {
                Default::default()
            };
            apply_chain(
                &store,
                ChainReorgArgs::for_test(detached, [block.clone()].into(), next),
            );
            let expected = match status {
                Status::Pending => [1, 0, 0],
                Status::Gap => [0, 1, 0],
                Status::Proposed => [0, 0, 1],
            };
            assert_entries(&store, expected);
            previous = Some(block);
        }

        let base = store.snapshot().1;
        let block = BlockBuilder::default()
            .number(base.tip_number() + 1)
            .parent_hash(base.tip_hash())
            .epoch(base.epoch_ext().number_with_fraction(base.tip_number() + 1))
            .transactions([tx(0), transaction.clone()])
            .build();
        let committed = snapshot(&store, &block, ProposalView::default());
        apply_chain(
            &store,
            ChainReorgArgs::for_test(Default::default(), [block.clone()].into(), committed),
        );
        assert!(store.point(&hash).1.is_none());
        assert_entries(&store, [0, 0, 0]);

        // Detachment installs a charged recovery candidate, not accepted status.
        apply_chain(
            &store,
            ChainReorgArgs::for_test([block].into(), Default::default(), base),
        );
        let recovery = store.point(&hash).1.unwrap();
        assert_eq!(recovery.source, Source::Recovery);
        assert!(matches!(recovery.phase, Phase::Resolve));
        assert_entries(&store, [0, 0, 0]);
        let (plan, reject) =
            admission(&store, &recovery, 1, 1, Status::Pending, &config()).unwrap();
        assert!(reject.is_none());
        store.apply(plan).unwrap();
        assert_entries(&store, [1, 0, 0]);
    }
}
