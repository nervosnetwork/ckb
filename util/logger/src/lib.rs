use ansi_term::{self, Colour};
use backtrace::Backtrace;
use chrono::prelude::{DateTime, Local};
use ckb_util::{Mutex, RwLock};
use crossbeam_channel::unbounded;
use env_logger::filter::{Builder, Filter};
use lazy_static::lazy_static;
use log::{LevelFilter, Log, Metadata, Record};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::{fs, panic, sync, thread};

pub use log::{self as internal, Level, SetLoggerError};
pub use serde_json::{json, Map, Value};

lazy_static! {
    static ref CONTROL_HANDLE: sync::Arc<RwLock<Option<crossbeam_channel::Sender<Message>>>> =
        sync::Arc::new(RwLock::new(None));
}

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
        $crate::internal::trace!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug {
    ($( $args:tt )*) => {
        $crate::internal::debug!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info {
    ($( $args:tt )*) => {
        $crate::internal::info!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn {
    ($( $args:tt )*) => {
        $crate::internal::warn!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error {
    ($( $args:tt )*) => {
        $crate::internal::error!($( $args )*);
    }
}

/// Enable metrics collection via configuring log filter. Here are some examples:
///   * `filter=info,ckb-metrics=trace`: enable all metrics collection
///   * `filter=info,ckb-metrics=trace,ckb-metrics-sync=off`: enable all metrics collection except
///     topic "sync"
///   * `filter=info,ckb-metrics-sync=trace`: only enable metrics collection of topic "sync"
#[macro_export(local_inner_macros)]
macro_rules! metric {
    ( { $( $args:tt )* } ) => {
        let topic = $crate::__metric_topic!( $($args)* );
        let filter = ::std::format!("ckb-metrics-{}", topic);
        if $crate::log_enabled_target!(&filter, $crate::Level::Trace) {
            let mut metric = $crate::json!( { $( $args )* });
            $crate::__log_metric(&mut metric, $crate::env!("CARGO_PKG_NAME"));
        }
    }
}

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! __metric_topic {
     ("topic" : $topic:expr , $($_args:tt)*) => {
         $topic
     };
     ($_key:literal : $_val:tt , $($args:tt)+) => {
        $crate::__metric_topic!($($args)+)
     }
 }

#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    ($level:expr) => {
        $crate::internal::log_enabled!($level);
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
    Filter(Filter),
    Terminate,
}

#[derive(Debug)]
pub struct Logger {
    sender: crossbeam_channel::Sender<Message>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
    filter: sync::Arc<RwLock<Filter>>,
    emit_sentry_breadcrumbs: bool,
}

#[cfg(target_os = "windows")]
fn enable_ansi_support() {
    ansi_term::enable_ansi_support()
        .unwrap_or_else(|code| println!("Cannot enable ansi support: {:?}", code));
}

#[cfg(not(target_os = "windows"))]
fn enable_ansi_support() {}

// Parse crate name leniently in logger filter: convert "-" to "_".
fn convert_compatible_crate_name(spec: &str) -> String {
    let mut parts = spec.splitn(2, '/');
    let first_part = parts.next();
    let last_part = parts.next();
    let mut mods = Vec::new();
    if let Some(mods_part) = first_part {
        for m in mods_part.split(',') {
            mods.push(m.to_owned());
            if m.contains('-') {
                mods.push(m.replace("-", "_"));
            }
        }
    }
    if let Some(filter) = last_part {
        [&mods.join(","), filter].join("/")
    } else {
        mods.join(",")
    }
}

#[test]
fn test_convert_compatible_crate_name() {
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace/*[0-9]";
    let expected = "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace/*[0-9]";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace";
    let expected =
        "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info/*[0-9]";
    let expected = "info/*[0-9]";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info";
    let expected = "info";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
}

impl Logger {
    fn new(config: Config) -> Logger {
        let mut builder = Builder::new();

        if let Ok(ref env_filter) = std::env::var("CKB_LOG") {
            builder.parse(&convert_compatible_crate_name(env_filter));
        } else if let Some(ref config_filter) = config.filter {
            builder.parse(&convert_compatible_crate_name(config_filter));
        }

        let (sender, receiver) = unbounded();
        CONTROL_HANDLE.write().replace(sender.clone());
        let Config {
            color,
            file,
            log_to_file,
            log_to_stdout,
            ..
        } = config;
        let file = if log_to_file { file } else { None };
        let filter = sync::Arc::new(RwLock::new(builder.build()));
        let filter_for_update = sync::Arc::clone(&filter);

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

                loop {
                    match receiver.recv() {
                        Ok(Message::Record(record)) => {
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
                        Ok(Message::Filter(filter)) => {
                            *filter_for_update.write() = filter;
                            log::set_max_level(filter_for_update.read().filter());
                        }
                        Ok(Message::Terminate) | Err(_) => {
                            break;
                        }
                    }
                }
            })
            .expect("Logger thread init should not fail");

        Logger {
            sender,
            handle: Mutex::new(Some(tb)),
            filter,
            emit_sentry_breadcrumbs: config.emit_sentry_breadcrumbs.unwrap_or_default(),
        }
    }

    pub fn filter(&self) -> LevelFilter {
        self.filter.read().filter()
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
        self.filter.read().enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Check if the record is matched by the filter
        if self.filter.read().matches(record) {
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
    let filter = logger.filter();
    log::set_boxed_logger(Box::new(logger)).map(|_| {
        log::set_max_level(filter);
        LoggerInitGuard
    })
}

pub fn silent() {
    log::set_max_level(LevelFilter::Off);
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

// Used inside macro `metric!`
pub fn __log_metric(metric: &mut Value, default_target: &str) {
    if metric.get("fields").is_none() {
        metric
            .as_object_mut()
            .map(|obj| obj.insert("fields".to_string(), Map::new().into()));
    }
    if metric.get("tags").is_none() {
        metric
            .as_object_mut()
            .map(|obj| obj.insert("tags".to_string(), Map::new().into()));
    }
    metric.get_mut("tags").and_then(|tags| {
        tags.as_object_mut().map(|tags| {
            if !tags.contains_key("target") {
                tags.insert("target".to_string(), default_target.into());
            }
        })
    });
    trace_target!("ckb-metrics", "{}", metric);
}

pub fn configure_logger_filter(filter_str: &str) {
    let filter = Builder::new()
        .parse(&convert_compatible_crate_name(filter_str))
        .build();
    let _ = CONTROL_HANDLE
        .read()
        .as_ref()
        .map(|sender| sender.send(Message::Filter(filter)));
}
