use crate::notifier::Notifier;
use ckb_core::alert::AlertBuilder;
use std::sync::Arc;

#[test]
fn test_notice_alerts_by_version() {
    let mut notifier = Notifier::new("0.9.0".into());
    let alert1 = Arc::new(
        AlertBuilder::default()
            .id(1)
            .max_version(Some("0.10.0".into()))
            .build(),
    );
    let alert2 = Arc::new(
        AlertBuilder::default()
            .id(2)
            .min_version(Some("0.10.0".into()))
            .build(),
    );
    notifier.add(alert1);
    notifier.add(alert2);
    assert_eq!(notifier.received_alerts().len(), 2);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    assert_eq!(notifier.noticed_alerts()[0].id, 1);
}

#[test]
fn test_received_alerts() {
    let mut notifier = Notifier::new("0.1.0".into());
    let alert1 = Arc::new(
        AlertBuilder::default()
            .id(1)
            .max_version(Some("0.2.0".into()))
            .min_version(Some("0.1.0".into()))
            .build(),
    );
    let dup_alert1 = Arc::new(AlertBuilder::default().id(1).build());
    notifier.add(Arc::clone(&alert1));
    assert!(notifier.has_received(1));
    notifier.add(dup_alert1);
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(notifier.received_alerts()[0].hash(), alert1.hash());
}

#[test]
fn test_cancel_alert() {
    let mut notifier = Notifier::new("0.1.0".into());
    let alert1 = Arc::new(
        AlertBuilder::default()
            .id(1)
            .max_version(Some("0.2.0".into()))
            .min_version(Some("0.1.0".into()))
            .build(),
    );
    let cancel_alert1 = Arc::new(AlertBuilder::default().id(2).cancel(1).build());
    notifier.add(Arc::clone(&alert1));
    assert!(notifier.has_received(1));
    notifier.add(Arc::clone(&cancel_alert1));
    assert!(notifier.has_received(1));
    assert!(notifier.has_received(2));
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    assert_eq!(notifier.received_alerts()[0].hash(), cancel_alert1.hash());
}

#[test]
fn test_clear_expired_alerts() {
    let mut notifier = Notifier::new("0.1.0".into());
    let notice_until = 1_561_084_974_000;
    let before_expired_time = notice_until - 1000;
    let after_expired_time = notice_until + 1000;
    let alert1 = Arc::new(
        AlertBuilder::default()
            .id(1)
            .notice_until(notice_until)
            .build(),
    );
    notifier.add(Arc::clone(&alert1));
    notifier.clear_expired_alerts(before_expired_time);
    assert!(notifier.has_received(1));
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    notifier.clear_expired_alerts(after_expired_time);
    assert!(!notifier.has_received(1));
    assert_eq!(notifier.received_alerts().len(), 0);
    assert_eq!(notifier.noticed_alerts().len(), 0);
}
