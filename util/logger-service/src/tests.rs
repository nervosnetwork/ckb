use std::path::Path;

use ckb_logger_config::LogFileSplit;
use time::OffsetDateTime;

use crate::{Logger, convert_compatible_crate_name};

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
fn test_log_file_split_path() {
    let timestamp = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let file_path = Path::new("/tmp/ckb/logs/run.log");

    assert_eq!(
        Logger::log_file_path(file_path, LogFileSplit::Never, timestamp),
        file_path
    );
    assert_eq!(
        Logger::log_file_path(file_path, LogFileSplit::Hourly, timestamp),
        Path::new("/tmp/ckb/logs/run.2023-11-14-22.log")
    );
    assert_eq!(
        Logger::log_file_path(file_path, LogFileSplit::Daily, timestamp),
        Path::new("/tmp/ckb/logs/run.2023-11-14.log")
    );
}
