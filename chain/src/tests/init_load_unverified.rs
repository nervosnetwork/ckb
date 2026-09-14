use super::*;
use crate::utils::orphan_block_pool::OrphanBlockPool;
use ckb_channel::bounded;
use ckb_shared::SharedBuilder;
use ckb_types::core::BlockBuilder;
use std::sync::atomic::Ordering;

#[test]
fn startup_recovery_does_not_requeue_blocks_received_after_capture() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let recovered = BlockBuilder::default()
        .number(1)
        .epoch(
            shared
                .consensus()
                .genesis_epoch_ext()
                .number_with_fraction(1),
        )
        .parent_hash(shared.consensus().genesis_hash())
        .build();
    let transaction = shared.store().begin_transaction();
    transaction.insert_block(&recovered).unwrap();
    transaction.commit().unwrap();
    shared.refresh_snapshot();

    let (requests, received) = bounded(4);
    let (truncate, _truncations) = bounded(1);
    let loading = Arc::new(AtomicBool::new(true));
    let controller = ChainController::new(
        requests,
        truncate,
        Arc::new(OrphanBlockPool::with_capacity(4)),
        Arc::clone(&loading),
    );
    let recovery = InitLoadUnverified::new(
        Arc::clone(&shared.snapshot()),
        controller,
        Arc::clone(&loading),
    );

    // A newly received block also has no BlockExt until verification commits.
    // It belongs to its original request, which may carry a verification switch
    // and callback; startup recovery must not enqueue a second unqualified copy.
    let incoming = BlockBuilder::default()
        .number(2)
        .epoch(
            shared
                .consensus()
                .genesis_epoch_ext()
                .number_with_fraction(2),
        )
        .parent_hash(recovered.hash())
        .build();
    let transaction = shared.store().begin_transaction();
    transaction.insert_block(&incoming).unwrap();
    transaction.commit().unwrap();
    assert!(shared.store().get_block_ext(&incoming.hash()).is_none());

    recovery.start();
    assert!(!loading.load(Ordering::Acquire));
    let replayed: Vec<_> = received
        .try_iter()
        .map(|request| {
            assert!(request.arguments.switch.is_none());
            assert!(request.arguments.verify_callback.is_none());
            request.arguments.block.hash()
        })
        .collect();
    assert_eq!(replayed, vec![recovered.hash()]);
}
