use crate::notifier::Notifier;
use ckb_async_runtime::{new_background_runtime, Handle};
use ckb_notify::NotifyService;
use ckb_stop_handler::StopHandler;
use ckb_types::{packed, prelude::*};
use once_cell::unsync;
use std::borrow::Borrow;

fn build_alert(
    id: u32,
    cancel: u32,
    min_ver: Option<&str>,
    max_ver: Option<&str>,
    notice_until: u64,
) -> packed::Alert {
    let raw = packed::RawAlert::new_builder()
        .id(id.pack())
        .cancel(cancel.pack())
        .min_version(min_ver.pack())
        .max_version(max_ver.pack())
        .notice_until(notice_until.pack())
        .build();
    packed::Alert::new_builder().raw(raw).build()
}

fn new_notifier(version: &str) -> Notifier {
    thread_local! {
        // NOTICE：we can't put the runtime directly into thread_local here,
        // on windows the runtime in thread_local will get stuck when dropping
        static RUNTIME_HANDLE: unsync::OnceCell<(Handle, StopHandler<()>)> = unsync::OnceCell::new();
    }

    let notify_controller = RUNTIME_HANDLE.with(|runtime| {
        NotifyService::new(
            Default::default(),
            runtime
                .borrow()
                .get_or_init(new_background_runtime)
                .0
                .clone(),
        )
        .start()
    });
    Notifier::new(version.into(), notify_controller)
}

#[test]
fn test_notice_alerts_by_version() {
    let mut notifier = new_notifier("0.9.0");
    let alert1 = build_alert(1, 0, None, Some("0.10.0"), 0);
    let alert2 = build_alert(2, 0, Some("0.10.0"), None, 0);
    notifier.add(&alert1);
    notifier.add(&alert2);
    assert_eq!(notifier.received_alerts().len(), 2);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    assert_eq!(
        notifier.noticed_alerts()[0].raw().id().as_slice(),
        &1u32.to_le_bytes()[..]
    );
}

#[test]
fn test_received_alerts() {
    let mut notifier = new_notifier("0.1.0");
    let alert1 = build_alert(1, 0, Some("0.1.0"), Some("0.2.0"), 0);
    let dup_alert1 = build_alert(1, 0, None, None, 0);
    notifier.add(&alert1);
    assert!(notifier.has_received(1));
    notifier.add(&dup_alert1);
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(
        notifier.received_alerts()[0].calc_alert_hash(),
        alert1.calc_alert_hash()
    );
}

#[test]
fn test_cancel_alert() {
    let mut notifier = new_notifier("0.1.0");
    let alert1 = build_alert(1, 0, Some("0.1.0"), Some("0.2.0"), 0);
    let cancel_alert1 = build_alert(2, 1, None, None, 0);
    notifier.add(&alert1);
    assert!(notifier.has_received(1));
    notifier.add(&cancel_alert1);
    assert!(notifier.has_received(1));
    assert!(notifier.has_received(2));
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    assert_eq!(
        notifier.received_alerts()[0].calc_alert_hash(),
        cancel_alert1.calc_alert_hash()
    );
}

#[test]
fn test_clear_expired_alerts() {
    let mut notifier = new_notifier("0.1.0");
    let notice_until = 1_561_084_974_000;
    let before_expired_time = notice_until - 1000;
    let after_expired_time = notice_until + 1000;
    let alert1 = build_alert(1, 0, None, None, notice_until);
    notifier.add(&alert1);
    notifier.clear_expired_alerts(before_expired_time);
    assert!(notifier.has_received(1));
    assert_eq!(notifier.received_alerts().len(), 1);
    assert_eq!(notifier.noticed_alerts().len(), 1);
    notifier.clear_expired_alerts(after_expired_time);
    assert!(!notifier.has_received(1));
    assert_eq!(notifier.received_alerts().len(), 0);
    assert_eq!(notifier.noticed_alerts().len(), 0);
}
