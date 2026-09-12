//! Stream committed refusal evidence into the runner's retained native log.
//! This observer stores no transaction data or unbounded in-memory history.

use ckb_logger::internal::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct RejectionLogger {
    records: AtomicUsize,
    service_records: AtomicUsize,
    write_failed: AtomicBool,
}
static LOGGER: RejectionLogger = RejectionLogger {
    records: AtomicUsize::new(0),
    service_records: AtomicUsize::new(0),
    write_failed: AtomicBool::new(false),
};

impl Log for RejectionLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "ckb_tx_pool::rejection" || metadata.level() <= ckb_logger::Level::Warn
    }
    fn log(&self, record: &Record<'_>) {
        if record.target() == "ckb_tx_pool::rejection" {
            if writeln!(
                std::io::stderr().lock(),
                "BENCH_REJECTION {}",
                record.args()
            )
            .is_err()
            {
                self.write_failed.store(true, Ordering::SeqCst);
            }
            self.records.fetch_add(1, Ordering::SeqCst);
        } else if self.enabled(record.metadata()) {
            let mut message = record.args().to_string();
            message.truncate(message.floor_char_boundary(8192));
            let observation = serde_json::json!({
                "level": record.level().as_str(), "target": record.target(), "message": message,
            });
            if writeln!(std::io::stderr().lock(), "BENCH_SERVICE_LOG {observation}").is_err() {
                self.write_failed.store(true, Ordering::SeqCst);
            }
            self.service_records.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn flush(&self) {}
}

/// Outlives the benchmark runtime, including cleanup after an early failure.
pub(super) struct Capture;

impl Drop for Capture {
    fn drop(&mut self) {
        if writeln!(
            std::io::stderr().lock(),
            "BENCH_REJECTION_CAPTURE {}",
            observation()
        )
        .is_err()
        {
            LOGGER.write_failed.store(true, Ordering::SeqCst);
        }
    }
}

pub(super) fn install() -> Result<Capture, SetLoggerError> {
    ckb_logger::internal::set_logger(&LOGGER)?;
    ckb_logger::internal::set_max_level(LevelFilter::Debug);
    Ok(Capture)
}

pub(super) fn observation() -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "logger": "rejections_and_warnings_v1",
        "records": LOGGER.records.load(Ordering::SeqCst),
        "service_records": LOGGER.service_records.load(Ordering::SeqCst),
        "write_failed": LOGGER.write_failed.load(Ordering::SeqCst),
    })
}

pub(super) fn check() -> Result<(), std::io::Error> {
    if LOGGER.write_failed.load(Ordering::SeqCst) {
        Err(std::io::Error::other("rejection diagnostic output failed"))
    } else {
        Ok(())
    }
}
