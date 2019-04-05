use ansi_term::Colour;
use chrono::prelude::{DateTime, Local};
use crossbeam_channel::unbounded;
use env_logger::filter::{Builder, Filter};
use lazy_static::lazy_static;
use log::{LevelFilter, SetLoggerError};
use log::{Log, Metadata, Record};
use parking_lot::Mutex;
use regex::Regex;
use serde_derive::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::{fs, thread};

enum Message {
    Record(String),
    Terminate,
}

#[derive(Debug)]
pub struct Logger {
    sender: crossbeam_channel::Sender<Message>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
    filter: Filter,
}

impl Logger {
    fn new(config: Config) -> Logger {
        let mut builder = Builder::new();

        if let Ok(ref env_filter) = std::env::var("NERVOS_LOG") {
            builder.parse(env_filter);
        }

        if let Some(ref config_filter) = config.filter {
            builder.parse(config_filter);
        }

        let (sender, receiver) = unbounded();
        let file = config.file;
        let enable_color = config.color;
        let copy_to_stdout = config.copy_to_stdout;

        let tb = thread::Builder::new()
            .name("LogWriter".to_owned())
            .spawn(move || {
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
                            let output = if enable_color {
                                record
                            } else {
                                removed_color.clone()
                            };
                            if let Some(mut file) = file.as_ref() {
                                let _ = file.write_all(removed_color.as_bytes());
                                let _ = file.write_all(b"\n");
                            };
                            if copy_to_stdout {
                                println!("{}", output);
                            }
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
            filter: builder.build(),
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
    pub copy_to_stdout: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            filter: None,
            color: !cfg!(windows),
            file: None,
            copy_to_stdout: true,
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
    let logger = Logger::new(config);
    log::set_max_level(logger.filter());
    log::set_boxed_logger(Box::new(logger)).map(|_| LoggerInitGuard)
}

pub fn flush() {
    log::logger().flush()
}
