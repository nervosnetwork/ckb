mod utils;

use std::path::Path;

use ckb_logger::Level;
use ckb_logger_config::LogFileSplit;

#[test]
fn log_file_split_hourly() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        config.file = Path::new("split.log").to_path_buf();
        config.log_file_split = LogFileSplit::Hourly;
        config.log_to_stdout = false;
    });
    let log_dir = config.log_dir.clone();
    let base_log_file = log_dir.join(config.file.as_path());
    let line_content = "test hourly log file split";

    utils::do_tests(config, || {
        ckb_logger::error!("{line_content}");
    });

    assert!(
        !base_log_file.exists(),
        "base log file [{}] should not be created when splitting is enabled",
        base_log_file.display()
    );

    let split_log_files = std::fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("split.") && name.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    assert_eq!(
        split_log_files.len(),
        1,
        "expected one split log file in [{}]",
        log_dir.display()
    );
    assert!(
        utils::has_line_in_log_file(&split_log_files[0], Level::Error, line_content),
        "line [{}] isn't found in the log [{}]",
        line_content,
        split_log_files[0].display()
    );
}
