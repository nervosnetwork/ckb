use ansi_term::{self, Colour};
use backtrace::Backtrace;
use chrono::prelude::{DateTime, Local};
use crossbeam_channel::unbounded;
use env_logger::filter::{Builder, Filter};
use lazy_static::lazy_static;
use log::{LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::{fs, panic, thread};

pub use log::{self as internal, Level, SetLoggerError};
pub use serde_json::json;

#[doc(hidden)]
#[macro_export]
macro_rules! env {
    ($($inner:tt)*) => {
        env!($($inner)*)
    }
}

#[macro_export(local_inner_macros)]
macro_rules! trace {
    ($( $args:tt )*) => {
        $crate::internal::trace!(target: $crate::env!("CARGO_PKG_NAME"), $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug {
    ($( $args:tt )*) => {
        $crate::internal::debug!(target: $crate::env!("CARGO_PKG_NAME"), $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info {
    ($( $args:tt )*) => {
        $crate::internal::info!(target: $crate::env!("CARGO_PKG_NAME"), $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn {
    ($( $args:tt )*) => {
        $crate::internal::warn!(target: $crate::env!("CARGO_PKG_NAME"), $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error {
    ($( $args:tt )*) => {
        $crate::internal::error!(target: $crate::env!("CARGO_PKG_NAME"), $( $args )*);
    }
}

/// Enable metrics collection feature by setting `ckb-metrics=trace` in logger filter.
#[macro_export(local_inner_macros)]
macro_rules! metric {
    ($( $args:tt )*) => {
        let mut obj = $crate::json!($( $args )*);
        obj.get_mut("tags")
            .and_then(|tags| {
                tags.as_object_mut()
                    .map(|tags|
                        if !tags.contains_key("target") {
                            tags.insert(String::from("target"), $crate::env!("CARGO_PKG_NAME").into());
                        }
                    )
            });
        $crate::internal::trace!(target: "ckb-metrics", "{}", obj);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    ($level:expr) => {
        $crate::internal::log_enabled!(target: $crate::env!("CARGO_PKG_NAME"), $level);
    };
}

#[macro_export(local_inner_macros)]
macro_rules! trace_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::trace!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::debug!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::info!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::warn!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::error!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! log_enabled_target {
    ($target:expr, $level:expr) => {
        $crate::internal::log_enabled!(target: $target, $level);
    };
}

enum Message {
    Record(String),
    Terminate,
}

#[derive(Debug)]
pub struct Logger {
    sender: crossbeam_channel::Sender<Message>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
    filter: Filter,
    emit_sentry_breadcrumbs: bool,
}

#[cfg(target_os = "windows")]
fn enable_ansi_support() {
    ansi_term::enable_ansi_support()
        .unwrap_or_else(|code| println!("Cannot enable ansi support: {:?}", code));
}

#[cfg(not(target_os = "windows"))]
fn enable_ansi_support() {}

impl Logger {
    fn new(config: Config) -> Logger {
        let mut builder = Builder::new();

        if let Ok(ref env_filter) = std::env::var("CKB_LOG") {
            builder.parse(env_filter);
        } else if let Some(ref config_filter) = config.filter {
            builder.parse(config_filter);
        }

        let (sender, receiver) = unbounded();
        let Config {
            color,
            file,
            log_to_file,
            log_to_stdout,
            ..
        } = config;
        let file = if log_to_file { file } else { None };

        let tb = thread::Builder::new()
            .name("LogWriter".to_owned())
            .spawn(move || {
                enable_ansi_support();

                let file = file.map(|file| {
                    fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&file)
                        .unwrap_or_else(|_| {
                            panic!("Cannot write to log file given: {:?}", file.as_os_str())
                        })
                });

                while let Ok(Message::Record(record)) = receiver.recv() {
                    let removed_color = sanitize_color(record.as_ref());
                    let output = if color { record } else { removed_color.clone() };
                    if let Some(mut file) = file.as_ref() {
                        let _ = file.write_all(removed_color.as_bytes());
                        let _ = file.write_all(b"\n");
                    };
                    if log_to_stdout {
                        println!("{}", output);
                    }
                }
            })
            .expect("Logger thread init should not fail");

        Logger {
            sender,
            handle: Mutex::new(Some(tb)),
            filter: builder.build(),
            emit_sentry_breadcrumbs: config.emit_sentry_breadcrumbs.unwrap_or_default(),
        }
    }

    pub fn filter(&self) -> LevelFilter {
        self.filter.filter()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub filter: Option<String>,
    pub color: bool,
    pub file: Option<PathBuf>,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
    pub emit_sentry_breadcrumbs: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            filter: None,
            color: !cfg!(windows),
            file: None,
            log_to_file: false,
            log_to_stdout: true,
            emit_sentry_breadcrumbs: None,
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Check if the record is matched by the filter
        if self.filter.matches(record) {
            if self.emit_sentry_breadcrumbs {
                use sentry::{add_breadcrumb, integrations::log::breadcrumb_from_record};
                add_breadcrumb(|| breadcrumb_from_record(record));
            }

            let thread = thread::current();
            let thread_name = thread.name().unwrap_or_default();

            let with_color = {
                let thread_name = format!("{}", Colour::Blue.bold().paint(thread_name));
                let dt: DateTime<Local> = Local::now();
                let timestamp = dt.format("%Y-%m-%d %H:%M:%S%.3f %Z").to_string();
                format!(
                    "{} {} {} {}  {}",
                    Colour::Black.bold().paint(timestamp),
                    thread_name,
                    record.level(),
                    record.target(),
                    record.args()
                )
            };
            let _ = self.sender.send(Message::Record(with_color));
        }
    }

    fn flush(&self) {
        let handle = self.handle.lock().take().expect("Logger flush only once");
        let _ = self.sender.send(Message::Terminate);
        let _ = handle.join();
    }
}

fn sanitize_color(s: &str) -> String {
    lazy_static! {
        static ref RE: Regex = Regex::new("\x1b\\[[^m]+m").expect("Regex compile success");
    }
    RE.replace_all(s, "").to_string()
}

/// Flush the logger when dropped
#[must_use]
pub struct LoggerInitGuard;

impl Drop for LoggerInitGuard {
    fn drop(&mut self) {
        flush();
    }
}

pub fn init(config: Config) -> Result<LoggerInitGuard, SetLoggerError> {
    setup_panic_logger();

    let logger = Logger::new(config);
    log::set_max_level(logger.filter());
    log::set_boxed_logger(Box::new(logger)).map(|_| LoggerInitGuard)
}

pub fn flush() {
    log::logger().flush()
}

// Replace the default panic hook with logger hook, which prints panic info into logfile.
// This function will replace all hooks that was previously registered, so make sure involving
// before other register operations.
fn setup_panic_logger() {
    let panic_logger = |info: &panic::PanicInfo| {
        let backtrace = Backtrace::new();
        let thread = thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let location = info.location().unwrap(); // The current implementation always returns Some
        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => *s,
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => &*s,
                None => "Box<Any>",
            },
        };
        log::error!(
            target: "panic", "thread '{}' panicked at '{}': {}:{}{:?}",
            name,
            msg,
            location.file(),
            location.line(),
            backtrace,
        );
    };
    panic::set_hook(Box::new(panic_logger));
}
