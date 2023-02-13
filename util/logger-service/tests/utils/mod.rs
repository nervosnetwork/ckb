#![allow(dead_code)]

use std::{
    fs::OpenOptions,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
};

use ckb_logger::Level;
use ckb_logger_config::{Config, ExtraLoggerConfig};
use tempfile::TempDir;

const DEFAULT_LOG_FILE: &str = "default.log";
const DEFAULT_LOG_ENV: &str = "DEFAULT_LOG_ENV";
const LOG_LEVELS: &[Level] = &[
    Level::Trace,
    Level::Debug,
    Level::Info,
    Level::Warn,
    Level::Error,
];
const LOG_TIMESTAMP_REGEX: &str =
    r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}([.]\d{1,3}) [+-]\d{2}:\d{2}";

pub fn has_line_in_log_file(log_file: &Path, log_level: Level, content_pattern: &str) -> bool {
    let full_line_pattern =
        format!(r"^{LOG_TIMESTAMP_REGEX} [^\s]+ {log_level} [^\s]+  {content_pattern}$",);
    let regex = regex::Regex::new(&full_line_pattern).unwrap();
    let file = OpenOptions::new().read(true).open(log_file).unwrap();
    for line in BufReader::new(file).lines() {
        let line_str = line.unwrap();
        if regex.is_match(&line_str) {
            return true;
        }
    }
    false
}

pub fn test_if_log_file_exists(log_file: &Path, should_exist: bool) {
    if should_exist {
        assert!(
            log_file.exists(),
            "log file [{}] should exist",
            log_file.display()
        );
    } else {
        assert!(
            !log_file.exists(),
            "log file [{}] shouldn't exist",
            log_file.display()
        );
    }
}

pub fn do_tests<F>(config: Config, func: F)
where
    F: Fn(),
{
    let guard = ckb_logger_service::init(None, config).unwrap();
    func();
    drop(guard);
}

pub fn do_tests_with_env<F>(env_filter: &str, config: Config, func: F)
where
    F: Fn(),
{
    std::env::set_var(DEFAULT_LOG_ENV, env_filter);
    let guard = ckb_logger_service::init(Some(DEFAULT_LOG_ENV), config).unwrap();
    func();
    drop(guard);
}

pub fn do_tests_with_silent_logger<F>(func: F)
where
    F: Fn(),
{
    let guard = ckb_logger_service::init_silent().unwrap();
    func();
    drop(guard);
}

pub fn config_in_tempdir<F>(func: F) -> (Config, TempDir)
where
    F: Fn(&mut Config),
{
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let mut config = Config {
        filter: Some(Level::Trace.as_str().to_owned()),
        file: Path::new(DEFAULT_LOG_FILE).to_path_buf(),
        log_dir: tmp_dir.path().to_path_buf(),
        log_to_file: true,
        log_to_stdout: true,
        ..Default::default()
    };
    func(&mut config);
    (config, tmp_dir)
}

pub fn output_log_for_all_log_levels(log_message: &str) {
    ckb_logger::error!("{}", log_message);
    ckb_logger::warn!("{}", log_message);
    ckb_logger::info!("{}", log_message);
    ckb_logger::debug!("{}", log_message);
    ckb_logger::trace!("{}", log_message);
}

pub fn all_log_levels() -> &'static [Level] {
    LOG_LEVELS
}

pub fn update_extra_logger(config: &mut Config, name: &str, filter: &str) {
    let value = ExtraLoggerConfig {
        filter: filter.to_owned(),
    };
    config.extra.insert(name.to_owned(), value);
}

pub fn extra_logger_file(log_dir: &Path, logger_name: &str) -> PathBuf {
    log_dir.join(format!("{logger_name}.log"))
}

pub fn apply_new_config() {
    // waiting for the new configuration to be applied.
    std::thread::sleep(std::time::Duration::from_secs(1));
}

pub fn test_log_to_file(enabled: bool) {
    let (config, _tmp_dir) = config_in_tempdir(|config| {
        let file_name = format!("test_log_to_file_{enabled}.log");
        config.file = Path::new(&file_name).to_path_buf();
        config.log_to_file = enabled;
    });
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = format!("test log_to_file = {enabled}");
    do_tests(config, || {
        ckb_logger::error!("{line_content}");
    });

    test_if_log_file_exists(&log_file, enabled);

    if enabled {
        assert!(
            has_line_in_log_file(&log_file, Level::Error, &line_content),
            "line [{}] isn't found in the log [{}]",
            line_content,
            log_file.display()
        );
    }
}
