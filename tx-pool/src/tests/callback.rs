use super::{Callbacks, in_callback, mark_callback_thread, with_callback_context};
use crate::component::entry::TxEntry;
use ckb_types::core::{Capacity, TransactionBuilder};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn publish_dispatches_the_typed_event() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut callbacks = Callbacks::new();
    let observed = Arc::clone(&calls);
    callbacks.register_pending(Box::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let entry = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        0,
        Capacity::zero(),
        0,
    );

    callbacks.publish(&super::CallbackEvent::Pending(entry.into()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn legacy_callback_worker_marker_is_thread_local() {
    assert!(!in_callback());
    std::thread::spawn(|| {
        mark_callback_thread();
        assert!(in_callback());
    })
    .join()
    .unwrap();
    assert!(!in_callback());
}

#[test]
fn scoped_callback_marker_restores_the_reused_thread() {
    assert!(!in_callback());
    with_callback_context(|| {
        assert!(in_callback());
        with_callback_context(|| assert!(in_callback()));
        assert!(in_callback());
    });
    assert!(!in_callback());
}
