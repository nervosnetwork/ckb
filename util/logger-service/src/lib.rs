//! CKB logger and logging service.

use ansi_term::Colour;
use backtrace::Backtrace;
use chrono::prelude::{DateTime, Local};
use ckb_channel::{self, unbounded};
use env_logger::filter::{Builder, Filter};
use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use once_cell::sync::OnceCell;
use regex::Regex;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::{fs, panic, process, sync, thread};

use ckb_logger_config::Config;
use ckb_util::{strings, Mutex, RwLock};

static CONTROL_HANDLE: OnceCell<ckb_channel::Sender<Message>> = OnceCell::new();
static RE: OnceCell<regex::Regex> = OnceCell::new();

enum Message {
    Record {
        is_match: bool,
        extras: Vec<String>,
        data: String,
    },
    UpdateMainLogger {
        filter: Option<Filter>,
        to_stdout: Option<bool>,
        to_file: Option<bool>,
        color: Option<bool>,
    },
    UpdateExtraLogger(String, Filter),
    RemoveExtraLogger(String),
    Terminate,
}

/// The CKB logger which implements [log::Log].
///
/// When a CKB logger is created, a logging service will be started in a background thread.
///
/// [log::Log]: https://docs.rs/log/*/log/trait.Log.html
#[derive(Debug)]
pub struct Logger {
    sender: ckb_channel::Sender<Message>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
    filter: sync::Arc<RwLock<Filter>>,
    emit_sentry_breadcrumbs: bool,
    extra_loggers: sync::Arc<RwLock<HashMap<String, ExtraLogger>>>,
}

#[derive(Debug)]
struct MainLogger {
    file_path: PathBuf,
    file: Option<fs::File>,
    to_stdout: bool,
    to_file: bool,
    color: bool,
}

#[derive(Debug)]
struct ExtraLogger {
    filter: Filter,
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
        for name in config.extra.keys() {
            if let Err(err) = Self::check_extra_logger_name(name) {
                eprintln!("Error: {}", err);
                process::exit(1);
            }
        }

        let (sender, receiver) = unbounded();
        CONTROL_HANDLE
            .set(sender.clone())
            .expect("CONTROL_HANDLE init once");

        let Config {
            color,
            file: file_path,
            log_dir,
            log_to_file,
            log_to_stdout,
            ..
        } = config;
        let mut main_logger = {
            let file = if log_to_file {
                match Self::open_log_file(&file_path) {
                    Err(err) => {
                        eprintln!("Error: {}", err);
                        process::exit(1);
                    }
                    Ok(file) => Some(file),
                }
            } else {
                None
            };
            MainLogger {
                file_path,
                file,
                to_stdout: log_to_stdout,
                to_file: log_to_file,
                color,
            }
        };

        let filter = {
            let filter = if let Ok(ref env_filter) = std::env::var("CKB_LOG") {
                Self::build_filter(env_filter)
            } else if let Some(ref config_filter) = config.filter {
                Self::build_filter(config_filter)
            } else {
                Self::build_filter("")
            };
            sync::Arc::new(RwLock::new(filter))
        };
        let filter_for_update = sync::Arc::clone(&filter);

        let extra_loggers = {
            let extra_loggers = config
                .extra
                .iter()
                .map(|(name, extra)| {
                    let filter = Self::build_filter(&extra.filter);
                    let extra_logger = ExtraLogger { filter };
                    (name.to_owned(), extra_logger)
                })
                .collect::<HashMap<_, _>>();
            sync::Arc::new(RwLock::new(extra_loggers))
        };
        let extra_loggers_for_update = sync::Arc::clone(&extra_loggers);

        let mut extra_files = {
            let extra_files_res = config
                .extra
                .keys()
                .map(|name| {
                    let file_path = log_dir.clone().join(name.to_owned() + ".log");
                    Self::open_log_file(&file_path).map(|file| (name.to_owned(), file))
                })
                .collect::<Result<HashMap<_, _>, _>>();
            if let Err(err) = extra_files_res {
                eprintln!("Error: {}", err);
                process::exit(1);
            }
            extra_files_res.unwrap()
        };

        let tb = thread::Builder::new()
            .name("LogWriter".to_owned())
            .spawn(move || {
                enable_ansi_support();

                loop {
                    match receiver.recv() {
                        Ok(Message::Record {
                            is_match,
                            extras,
                            data,
                        }) => {
                            let removed_color = if (is_match
                                && (!main_logger.color || main_logger.to_file))
                                || !extras.is_empty()
                            {
                                sanitize_color(data.as_ref())
                            } else {
                                "".to_owned()
                            };
                            if is_match {
                                if main_logger.to_stdout {
                                    let output = if main_logger.color {
                                        data.as_str()
                                    } else {
                                        removed_color.as_str()
                                    };
                                    println!("{}", output);
                                }
                                if main_logger.to_file {
                                    if let Some(mut file) = main_logger.file.as_ref() {
                                        let _ = file.write_all(removed_color.as_bytes());
                                        let _ = file.write_all(b"\n");
                                    };
                                }
                            }
                            for name in extras {
                                if let Some(mut file) = extra_files.get(&name) {
                                    let _ = file.write_all(removed_color.as_bytes());
                                    let _ = file.write_all(b"\n");
                                }
                            }
                            continue;
                        }
                        Ok(Message::UpdateMainLogger {
                            filter,
                            to_stdout,
                            to_file,
                            color,
                        }) => {
                            if let Some(filter) = filter {
                                *filter_for_update.write() = filter;
                            }
                            if let Some(to_stdout) = to_stdout {
                                main_logger.to_stdout = to_stdout;
                            }
                            if let Some(to_file) = to_file {
                                main_logger.to_file = to_file;
                                if main_logger.to_file {
                                    if main_logger.file.is_none() {
                                        main_logger.file =
                                            Self::open_log_file(&main_logger.file_path).ok();
                                    }
                                } else {
                                    main_logger.file = None;
                                }
                            }
                            if let Some(color) = color {
                                main_logger.color = color;
                            }
                        }
                        Ok(Message::UpdateExtraLogger(name, filter)) => {
                            let file = log_dir.clone().join(name.clone() + ".log");
                            let file_res = Self::open_log_file(&file);
                            if let Ok(file) = file_res {
                                extra_files.insert(name.clone(), file);
                                extra_loggers_for_update
                                    .write()
                                    .insert(name, ExtraLogger { filter });
                            }
                        }
                        Ok(Message::RemoveExtraLogger(name)) => {
                            extra_loggers_for_update.write().remove(&name);
                            extra_files.remove(&name);
                        }
                        Ok(Message::Terminate) | Err(_) => {
                            break;
                        }
                    }
                    let max_level = Self::max_level_filter(
                        &filter_for_update.read(),
                        &extra_loggers_for_update.read(),
                    );
                    log::set_max_level(max_level);
                }
            })
            .expect("Logger thread init should not fail");

        Logger {
            sender,
            handle: Mutex::new(Some(tb)),
            filter,
            emit_sentry_breadcrumbs: config.emit_sentry_breadcrumbs.unwrap_or_default(),
            extra_loggers,
        }
    }

    fn open_log_file(file_path: &PathBuf) -> Result<fs::File, String> {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .map_err(|err| {
                format!(
                    "Cannot write to log file given: {:?} since {}",
                    file_path.as_os_str(),
                    err
                )
            })
    }

    fn build_filter(filter_str: &str) -> Filter {
        Builder::new()
            .parse(&convert_compatible_crate_name(filter_str))
            .build()
    }

    fn max_level_filter(
        main_filter: &Filter,
        extra_loggers: &HashMap<String, ExtraLogger>,
    ) -> LevelFilter {
        extra_loggers
            .values()
            .fold(main_filter.filter(), |ret, curr| {
                ret.max(curr.filter.filter())
            })
    }

    fn filter(&self) -> LevelFilter {
        Self::max_level_filter(&self.filter.read(), &self.extra_loggers.read())
    }

    fn send_message(message: Message) -> Result<(), String> {
        CONTROL_HANDLE
            .get()
            .ok_or_else(|| "no sender for logger service".to_owned())
            .and_then(|sender| {
                sender
                    .send(message)
                    .map_err(|err| format!("failed to send message to logger service: {}", err))
                    .map(|_| ())
            })
    }

    /// Updates the main logger.
    pub fn update_main_logger(
        filter_str: Option<String>,
        to_stdout: Option<bool>,
        to_file: Option<bool>,
        color: Option<bool>,
    ) -> Result<(), String> {
        let filter = filter_str.map(|s| Self::build_filter(&s));
        let message = Message::UpdateMainLogger {
            filter,
            to_stdout,
            to_file,
            color,
        };
        Self::send_message(message)
    }

    /// Checks if the input extra logger name is valid.
    pub fn check_extra_logger_name(name: &str) -> Result<(), String> {
        strings::check_if_identifier_is_valid(name)
    }

    /// Updates an extra logger through it's name.
    pub fn update_extra_logger(name: String, filter_str: String) -> Result<(), String> {
        let filter = Self::build_filter(&filter_str);
        let message = Message::UpdateExtraLogger(name, filter);
        Self::send_message(message)
    }

    /// Removes an extra logger.
    pub fn remove_extra_logger(name: String) -> Result<(), String> {
        let message = Message::RemoveExtraLogger(name);
        Self::send_message(message)
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter.read().enabled(metadata)
            || self
                .extra_loggers
                .read()
                .values()
                .any(|logger| logger.filter.enabled(metadata))
    }

    fn log(&self, record: &Record) {
        // Check if the record is matched by the main filter
        let is_match = self.filter.read().matches(record);
        let extras = self
            .extra_loggers
            .read()
            .iter()
            .filter_map(|(name, logger)| {
                if logger.filter.matches(record) {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if is_match || !extras.is_empty() {
            if self.emit_sentry_breadcrumbs {
                use sentry::{add_breadcrumb, integrations::log::breadcrumb_from_record};
                add_breadcrumb(|| breadcrumb_from_record(record));
            }

            let thread = thread::current();
            let thread_name = thread.name().unwrap_or("*unnamed*");

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
            let _ = self.sender.send(Message::Record {
                is_match,
                extras,
                data: with_color,
            });
        }
    }

    fn flush(&self) {
        let handle = self.handle.lock().take().expect("Logger flush only once");
        let _ = self.sender.send(Message::Terminate);
        let _ = handle.join();
    }
}

fn sanitize_color(s: &str) -> String {
    let re = RE.get_or_init(|| Regex::new("\x1b\\[[^m]+m").expect("Regex compile success"));
    re.replace_all(s, "").to_string()
}

/// Flushes the logger when dropped.
#[must_use]
pub struct LoggerInitGuard;

impl Drop for LoggerInitGuard {
    fn drop(&mut self) {
        flush();
    }
}

/// Initializes the [Logger](struct.Logger.html) and run the logging service.
pub fn init(config: Config) -> Result<LoggerInitGuard, SetLoggerError> {
    setup_panic_logger();

    let logger = Logger::new(config);
    let filter = logger.filter();
    log::set_boxed_logger(Box::new(logger)).map(|_| {
        log::set_max_level(filter);
        LoggerInitGuard
    })
}

/// Flushes any buffered records.
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
