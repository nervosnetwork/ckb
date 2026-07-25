use super::{Callbacks, call_guarded, in_callback};
use crate::component::entry::TxEntry;
use ckb_types::core::{Capacity, TransactionBuilder};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn guarded_callback_panic_cannot_escape_or_block_later_callbacks() {
    let calls = AtomicUsize::new(0);

    call_guarded("test", || {
        calls.fetch_add(1, Ordering::SeqCst);
        panic!("injected callback panic");
    });
    call_guarded("test", || {
        calls.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

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
fn callback_context_is_scoped_across_panics_and_nested_calls() {
    assert!(!in_callback());
    call_guarded("outer", || {
        assert!(in_callback());
        // Unrelated threads must never inherit callback ancestry: chain
        // reorg delivery and ordinary RPC traffic remain authoritative
        // while this callback is running.
        std::thread::spawn(|| assert!(!in_callback()))
            .join()
            .unwrap();
        call_guarded("inner", || assert!(in_callback()));
        assert!(in_callback());
        panic!("injected callback panic");
    });
    assert!(!in_callback());
}
