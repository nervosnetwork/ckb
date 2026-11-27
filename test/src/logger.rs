use ckb_util::Mutex;
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static SPEC_LOG_SINK: RefCell<Option<BufWriter<File>>> = const { RefCell::new(None) };
}

/// RAII guard routing the current worker thread's `ckb_test` logs into `<spec_dir>/runner.log`.
pub(crate) struct SpecLogGuard {
    previous: Option<BufWriter<File>>,
}

impl SpecLogGuard {
    pub(crate) fn enter(path: &Path) -> Self {
        let writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(BufWriter::new);
        let previous = SPEC_LOG_SINK.with(|slot| slot.replace(writer));
        Self { previous }
    }
}

impl Drop for SpecLogGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SPEC_LOG_SINK.with(|slot| {
            if let Some(mut current) = slot.replace(previous) {
                let _ = current.flush();
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilterRules {
    default_level: LevelFilter,
    targets: Vec<(String, LevelFilter)>,
}

impl FilterRules {
    fn parse(spec: &str) -> Self {
        let mut default_level = LevelFilter::Info;
        let mut targets = Vec::new();
        for raw in spec.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some((target, level_str)) = entry.split_once('=') {
                let target = target.trim().replace('-', "_");
                if let Some(level) = parse_level(level_str.trim()) {
                    targets.push((target, level));
                }
            } else if let Some(level) = parse_level(entry) {
                default_level = level;
            } else {
                // Target without explicit level defaults to Debug, matching env_logger.
                targets.push((entry.replace('-', "_"), LevelFilter::Debug));
            }
        }
        // Sort longest prefix first so more specific directives win.
        targets.sort_by_key(|b| std::cmp::Reverse(b.0.len()));
        Self {
            default_level,
            targets,
        }
    }

    fn matches(&self, target: &str, level: Level) -> bool {
        let normalized = target.replace('-', "_");
        for (prefix, max_level) in &self.targets {
            if normalized == *prefix
                || normalized
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with("::"))
            {
                return level <= *max_level;
            }
        }
        level <= self.default_level
    }
}

fn parse_level(raw: &str) -> Option<LevelFilter> {
    match raw.to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

fn format_timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let secs_of_day = total_secs % 86_400;
    let hours = secs_of_day / 3600;
    let minutes = (secs_of_day % 3600) / 60;
    let secs = secs_of_day % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

fn format_record_line(record: &Record<'_>) -> String {
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    format!(
        "{} {:<5} {:<9} {}  {}",
        format_timestamp(),
        record.level(),
        thread_name,
        record.target(),
        record.args()
    )
}

struct TestLogger {
    rules: FilterRules,
    log_file: Option<Mutex<BufWriter<File>>>,
}

impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if self.rules.matches(metadata.target(), metadata.level()) {
            return true;
        }
        // Keep per-spec `runner.log` detailed even when console output is quiet.
        SPEC_LOG_SINK.with(|slot| {
            slot.borrow().is_some()
                && ((metadata.target().starts_with("ckb_test") && metadata.level() <= Level::Debug)
                    || metadata.level() <= Level::Info)
        })
    }

    fn log(&self, record: &Record<'_>) {
        let to_console = self.rules.matches(record.target(), record.level());
        let to_spec = SPEC_LOG_SINK.with(|slot| {
            slot.borrow().is_some()
                && ((record.target().starts_with("ckb_test") && record.level() <= Level::Debug)
                    || record.level() <= Level::Info)
        });
        if !to_console && !to_spec {
            return;
        }

        let line = format_record_line(record);

        if to_spec {
            SPEC_LOG_SINK.with(|slot| {
                if let Some(writer) = slot.borrow_mut().as_mut() {
                    let _ = writeln!(writer, "{line}");
                    if record.level() <= Level::Warn {
                        let _ = writer.flush();
                    }
                }
            });
        }

        if to_console {
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout, "{line}");
            if let Some(file_mutex) = &self.log_file {
                let mut writer = file_mutex.lock();
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
        }
    }

    fn flush(&self) {
        let _ = io::stdout().flush();
        if let Some(file_mutex) = &self.log_file {
            let _ = file_mutex.lock().flush();
        }
        SPEC_LOG_SINK.with(|slot| {
            if let Some(writer) = slot.borrow_mut().as_mut() {
                let _ = writer.flush();
            }
        });
    }
}

static LOGGER_INIT: OnceLock<()> = OnceLock::new();

pub(crate) fn init_test_logger(filter: &str, log_file: Option<&Path>) {
    LOGGER_INIT.get_or_init(|| {
        let rules = FilterRules::parse(filter);
        let file_sink = log_file.map(|path| {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                let _ = fs::create_dir_all(parent);
            }
            let file = File::create(path).unwrap_or_else(|err| {
                panic!("failed to create log file {}: {err}", path.display())
            });
            Mutex::new(BufWriter::new(file))
        });

        let logger = Box::new(TestLogger {
            rules,
            log_file: file_sink,
        });
        log::set_boxed_logger(logger).expect("set integration test logger");
        // Allow per-spec runner.log to capture Debug logs even when console filter is Warn.
        log::set_max_level(LevelFilter::Trace);

        // Route panics into per-spec runner.log (and console only if filter enables `panic`),
        // keeping parallel worker failure output clean while preserving full backtraces in files.
        panic::set_hook(Box::new(|info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let entry = format!("panicked at {location}: {msg}\n{backtrace}");
            let recorded_to_spec = SPEC_LOG_SINK.with(|slot| {
                if let Some(writer) = slot.borrow_mut().as_mut() {
                    let _ = writeln!(writer, "{} ERROR panic  {entry}", format_timestamp());
                    let _ = writer.flush();
                    true
                } else {
                    false
                }
            });
            if !recorded_to_spec {
                let mut stderr = io::stderr().lock();
                let _ = writeln!(stderr, "thread panicked at {location}: {msg}\n{backtrace}");
            }
        }));
    });
}

/// Read the tail of a spec's `runner.log` if it exists.
pub(crate) fn read_runner_log_path(spec_dir: &Path) -> Option<PathBuf> {
    let path = spec_dir.join("runner.log");
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::TempPathBuf;

    #[test]
    fn filter_rules_prefer_most_specific_target() {
        let rules = FilterRules::parse(
            "warn,ckb_network=error,ckb_test=warn,ckb_test::progress=info,ckb-sync=debug",
        );
        assert!(rules.matches("ckb_test::progress", Level::Info));
        assert!(!rules.matches("ckb_test::progress", Level::Debug));
        assert!(!rules.matches("ckb_test::node", Level::Info));
        assert!(rules.matches("ckb_test::node", Level::Warn));
        assert!(!rules.matches("ckb_network::peer", Level::Warn));
        assert!(rules.matches("ckb_network::peer", Level::Error));
        assert!(rules.matches("ckb_sync::types", Level::Debug));
        assert!(rules.matches("unrelated", Level::Warn));
        assert!(!rules.matches("unrelated", Level::Info));
    }

    #[test]
    fn spec_log_guard_routes_to_isolated_file() {
        let spec_dir = TempPathBuf::new_spec_dir("LoggerUnitTest").expect("create spec dir");
        let runner_log = spec_dir.path().join("runner.log");
        {
            let _guard = SpecLogGuard::enter(&runner_log);
            SPEC_LOG_SINK.with(|slot| {
                let mut borrow = slot.borrow_mut();
                let writer = borrow.as_mut().expect("sink active");
                writeln!(writer, "hello isolated spec log").unwrap();
            });
        }
        let content = fs::read_to_string(&runner_log).unwrap();
        assert!(content.contains("hello isolated spec log"));
    }
}
