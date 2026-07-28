use crate::{Message, convert_compatible_crate_name, try_send_message, try_send_record};
use ckb_channel::bounded;
use log::Level;

#[test]
fn test_convert_compatible_crate_name() {
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace/*[0-9]";
    let expected = "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace/*[0-9]";
    let result = convert_compatible_crate_name(spec);
    assert_eq!(&result, &expected);
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace";
    let expected =
        "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace";
    let result = convert_compatible_crate_name(spec);
    assert_eq!(&result, &expected);
    let spec = "info/*[0-9]";
    let expected = "info/*[0-9]";
    let result = convert_compatible_crate_name(spec);
    assert_eq!(&result, &expected);
    let spec = "info";
    let expected = "info";
    let result = convert_compatible_crate_name(spec);
    assert_eq!(&result, &expected);
}

#[test]
fn full_log_channel_drops_newest_record() {
    let (sender, receiver) = bounded(1);
    assert!(try_send_record(&sender, record_message("oldest")));
    assert!(!try_send_record(&sender, record_message("newest")));

    match receiver.try_recv().unwrap() {
        Message::Record {
            original_message, ..
        } => assert_eq!(original_message, "oldest"),
        _ => unreachable!(),
    }
    assert!(receiver.try_recv().is_err());
}

#[test]
fn full_log_channel_rejects_control_message_without_blocking() {
    let (sender, receiver) = bounded(1);
    assert!(try_send_record(&sender, record_message("oldest")));

    let result = try_send_message(&sender, Message::RemoveExtraLogger("test".to_owned()));
    assert!(result.is_err());

    match receiver.try_recv().unwrap() {
        Message::Record {
            original_message, ..
        } => assert_eq!(original_message, "oldest"),
        _ => unreachable!(),
    }
    assert!(receiver.try_recv().is_err());
}

fn record_message(original_message: &str) -> Message {
    Message::Record {
        is_match: true,
        extras: Vec::new(),
        data: original_message.to_owned(),
        level: Level::Info,
        target: "test".to_owned(),
        date: String::new(),
        original_message: original_message.to_owned(),
    }
}
