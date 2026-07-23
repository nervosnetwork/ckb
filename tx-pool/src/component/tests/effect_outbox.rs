use crate::component::effect_outbox::{
    EffectOutbox, EffectOutboxError, EffectOutboxLimits, EffectOutboxUsage,
};

#[test]
fn count_and_byte_limits_cover_reserved_queued_and_active_batches() {
    let mut outbox = EffectOutbox::new(EffectOutboxLimits::new(2, 10)).unwrap();
    let first = outbox.reserve(4).unwrap();
    let second = outbox.reserve(6).unwrap();
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 2,
            bytes: 10
        }
    );
    assert_eq!(
        outbox.reserve(0),
        Err(EffectOutboxError::BatchLimitExceeded)
    );

    outbox.commit_reserved(&first, 4, "first").unwrap();
    let sequence = outbox.checkout().unwrap().unwrap();
    assert_eq!(outbox.active_effect(sequence).unwrap(), &"first");
    // Checkout does not refund the terminal payload gap.
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 2,
            bytes: 10
        }
    );
    outbox.complete_active(sequence).unwrap();
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 1,
            bytes: 6
        }
    );
    outbox.cancel(&second).unwrap();
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 0,
            bytes: 0
        }
    );
    outbox.audit().unwrap();
}

#[test]
fn retry_retains_fifo_head_and_residency() {
    let mut outbox = EffectOutbox::new(EffectOutboxLimits::new(4, 100)).unwrap();
    for effect in ["first", "second"] {
        let reservation = outbox.reserve(10).unwrap();
        outbox.commit_reserved(&reservation, 10, effect).unwrap();
    }
    let first = outbox.checkout().unwrap().unwrap();
    assert_eq!(outbox.active_effect(first).unwrap(), &"first");
    outbox.retry_active(first).unwrap();
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 2,
            bytes: 20
        }
    );
    let retried = outbox.checkout().unwrap().unwrap();
    assert_eq!(retried, first);
    assert_eq!(outbox.complete_active(retried).unwrap(), "first");
    let second = outbox.checkout().unwrap().unwrap();
    assert_eq!(outbox.complete_active(second).unwrap(), "second");
    outbox.audit().unwrap();
}

#[test]
fn conservative_reservation_is_refunded_when_batch_is_committed() {
    let mut outbox = EffectOutbox::new(EffectOutboxLimits::new(2, 100)).unwrap();
    let reservation = outbox.reserve(100).unwrap();
    outbox.commit_reserved(&reservation, 10, "small").unwrap();
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 1,
            bytes: 10
        }
    );
    outbox.audit().unwrap();
}

#[test]
fn fifo_sequence_follows_authoritative_commit_not_reservation_order() {
    let mut outbox = EffectOutbox::new(EffectOutboxLimits::new(4, 100)).unwrap();
    let first = outbox.reserve(10).unwrap();
    let second = outbox.reserve(10).unwrap();
    let second_sequence = outbox.commit_reserved(&second, 10, "second").unwrap();
    let first_sequence = outbox.commit_reserved(&first, 10, "first").unwrap();
    assert!(second_sequence < first_sequence);
    let checked_out = outbox.checkout().unwrap().unwrap();
    assert_eq!(checked_out, second_sequence);
    assert_eq!(outbox.complete_active(checked_out).unwrap(), "second");
    let checked_out = outbox.checkout().unwrap().unwrap();
    assert_eq!(checked_out, first_sequence);
    assert_eq!(outbox.complete_active(checked_out).unwrap(), "first");
    outbox.audit().unwrap();
}

#[test]
fn injected_commit_failure_leaves_reservation_intact() {
    let mut outbox: EffectOutbox<&'static str> =
        EffectOutbox::new(EffectOutboxLimits::new(2, 100)).unwrap();
    let reservation = outbox.reserve(10).unwrap();
    outbox.set_next_sequence_for_test(u64::MAX);
    assert_eq!(
        outbox.commit_reserved(&reservation, 10, "never queued"),
        Err(EffectOutboxError::SequenceExhausted)
    );
    assert_eq!(outbox.queued_len(), 0);
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 1,
            bytes: 10
        }
    );
    outbox.cancel(&reservation).unwrap();
    outbox.audit().unwrap();
}

#[test]
fn production_reservation_preflights_sequence_capacity() {
    let mut outbox: EffectOutbox<&'static str> =
        EffectOutbox::new(EffectOutboxLimits::new(2, 100)).unwrap();
    outbox.set_next_sequence_for_test(u64::MAX);
    assert_eq!(
        outbox.reserve(10),
        Err(EffectOutboxError::SequenceExhausted)
    );
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 0,
            bytes: 0
        }
    );
}

#[test]
fn stalled_publisher_cannot_escape_the_global_outbox_budget() {
    let mut outbox = EffectOutbox::new(EffectOutboxLimits::new(3, 30)).unwrap();
    for effect in [1u8, 2, 3] {
        let reservation = outbox.reserve(10).unwrap();
        outbox.commit_reserved(&reservation, 10, effect).unwrap();
    }
    let active = outbox.checkout().unwrap().unwrap();
    assert_eq!(
        outbox.reserve(1),
        Err(EffectOutboxError::BatchLimitExceeded)
    );
    assert_eq!(
        outbox.usage(),
        EffectOutboxUsage {
            batches: 3,
            bytes: 30
        }
    );
    outbox.retry_active(active).unwrap();
    outbox.audit().unwrap();
}
