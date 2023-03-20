#![allow(missing_docs)]
//! A lightweight metrics facade used in CKB.
//!
//! The `ckb-metrics` crate is a set of tools for metrics.
//! The crate [`ckb-metrics-service`] is the runtime which handles the metrics data in CKB.
//!
//! [`ckb-metrics-service`]: ../ckb_metrics_service/index.html

use prometheus::{
    register_histogram_vec, register_int_counter, register_int_gauge, register_int_gauge_vec,
    HistogramVec, IntCounter, IntGauge, IntGaugeVec,
};
use prometheus_static_metric::make_static_metric;
use std::cell::Cell;

pub fn gather() -> Vec<prometheus::proto::MetricFamily> {
    prometheus::gather()
}

make_static_metric! {
    // Struct for the CKB sys mem process statistics type label
    struct CkbSysMemProcessStatistics: IntGauge{
        "type" => {
            rss,
            vms,
        },
    }

    // Struct for the CKB sys mem jemalloc statistics type label
    struct CkbSysMemJemallocStatistics: IntGauge{
        "type" => {
            allocated,
            resident,
            active,
            mapped,
            retained,
            metadata,
        },
    }
}

pub struct Metrics {
    /// Gauge metric for CKB chain tip header number
    pub ckb_chain_tip: IntGauge,
    /// Gauge for tracking the size of all frozen data
    pub ckb_freezer_size: IntGauge,
    /// Counter for measuring the effective amount of data read
    pub ckb_freezer_read: IntCounter,
    /// Counter for relay transaction short id collide
    pub ckb_relay_transaction_short_id_collide: IntCounter,
    /// Counter for relay compact block transaction count
    pub ckb_relay_cb_transaction_count: IntCounter,
    /// Counter for relay compact block reconstruct ok
    pub ckb_relay_cb_reconstruct_ok: IntCounter,
    /// Counter for relay compact block fresh transaction count
    pub ckb_relay_cb_fresh_tx_cnt: IntCounter,
    /// Counter for relay compact block reconstruct fail
    pub ckb_relay_cb_reconstruct_fail: IntCounter,
    // Gauge for CKB shared best number
    pub ckb_shared_best_number: IntGauge,
    // GaugeVec for CKB system memory process statistics
    pub ckb_sys_mem_process: CkbSysMemProcessStatistics,
    // GaugeVec for CKB system memory jemalloc statistics
    pub ckb_sys_mem_jemalloc: CkbSysMemJemallocStatistics,
    /// Histogram for CKB network connections
    pub ckb_message_bytes: HistogramVec,
    /// Gauge for CKB rocksdb statistics
    pub ckb_sys_mem_rocksdb: IntGaugeVec,
    /// Counter for CKB network ban peers
    pub ckb_network_ban_peer: IntCounter,
}

static METRICS: once_cell::sync::Lazy<Metrics> = once_cell::sync::Lazy::new(|| Metrics {
    ckb_chain_tip: register_int_gauge!("ckb_chain_tip", "The CKB chain tip header number").unwrap(),
    ckb_freezer_size: register_int_gauge!("ckb_freezer_size", "The CKB freezer size").unwrap(),
    ckb_freezer_read: register_int_counter!("ckb_freezer_read", "The CKB freezer read").unwrap(),
    ckb_relay_transaction_short_id_collide: register_int_counter!(
        "ckb_relay_transaction_short_id_collide",
        "The CKB relay transaction short id collide"
    )
    .unwrap(),
    ckb_relay_cb_transaction_count: register_int_counter!(
        "ckb_relay_cb_transaction_count",
        "The CKB relay compact block transaction count"
    )
    .unwrap(),
    ckb_relay_cb_reconstruct_ok: register_int_counter!(
        "ckb_relay_cb_reconstruct_ok",
        "The CKB relay compact block reconstruct ok count"
    )
    .unwrap(),
    ckb_relay_cb_fresh_tx_cnt: register_int_counter!(
        "ckb_relay_cb_fresh_tx_cnt",
        "The CKB relay compact block fresh tx count"
    )
    .unwrap(),
    ckb_relay_cb_reconstruct_fail: register_int_counter!(
        "ckb_relay_cb_reconstruct_fail",
        "The CKB relay compact block reconstruct fail count"
    )
    .unwrap(),
    ckb_shared_best_number: register_int_gauge!(
        "ckb_shared_best_number",
        "The CKB shared best header number"
    )
    .unwrap(),
    ckb_sys_mem_process: CkbSysMemProcessStatistics::from(
        &register_int_gauge_vec!(
            "ckb_sys_mem_process",
            "CKB system memory for process statistics",
            &["type"]
        )
        .unwrap(),
    ),
    ckb_sys_mem_jemalloc: CkbSysMemJemallocStatistics::from(
        &register_int_gauge_vec!(
            "ckb_sys_mem_jemalloc",
            "CKB system memory for jemalloc statistics",
            &["type"]
        )
        .unwrap(),
    ),
    ckb_message_bytes: register_histogram_vec!(
        "ckb_message_bytes",
        "The CKB message bytes",
        &["direction", "protocol_name", "msg_item_name", "status_code"],
        vec![
            500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0, 50000.0, 100000.0, 200000.0, 500000.0
        ]
    )
    .unwrap(),

    ckb_sys_mem_rocksdb: register_int_gauge_vec!(
        "ckb_sys_mem_rocksdb",
        "CKB system memory for rocksdb statistics",
        &["type", "cf"]
    )
    .unwrap(),
    ckb_network_ban_peer: register_int_counter!(
        "ckb_network_ban_peer",
        "CKB network baned peer count"
    )
    .unwrap(),
});

/// Indicate whether the metrics service is enabled.
/// This value will set by ckb-metrics-service
pub static METRICS_SERVICE_ENABLED: once_cell::sync::OnceCell<bool> =
    once_cell::sync::OnceCell::new();

thread_local! {
    static ENABLE_COLLECT_METRICS: Cell<Option<bool>>= Cell::default();
}

/// if metrics service is enabled, `handle()` will return `Some(&'static METRICS)`
/// else will return `None`
pub fn handle() -> Option<&'static Metrics> {
    let enabled_collect_metrics: bool =
        ENABLE_COLLECT_METRICS.with(
            |enable_collect_metrics| match enable_collect_metrics.get() {
                Some(enabled) => enabled,
                None => match METRICS_SERVICE_ENABLED.get().copied() {
                    Some(enabled) => {
                        enable_collect_metrics.set(Some(enabled));
                        enabled
                    }
                    None => false,
                },
            },
        );

    if enabled_collect_metrics {
        Some(&METRICS)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::METRICS;
    use std::ops::Deref;

    // https://prometheus.io/docs/concepts/data_model/#metric-names-and-labels
    // The Metric names may contain ASCII letters and digits, as well as underscores and colons. It must match the regex [a-zA-Z_:][a-zA-Z0-9_:]*.
    // The Metric Label names may contain ASCII letters, numbers, as well as underscores. They must match the regex [a-zA-Z_][a-zA-Z0-9_]*. Label names beginning with __ are reserved for internal use.
    // Test that all metrics have valid names and labels
    // Just simple call .deref() method to make sure all metrics are initialized successfully
    // If the metrics name or label is invalid, this test will panic
    #[test]
    fn test_metrics_name() {
        let _ = METRICS.deref();
    }

    #[test]
    #[should_panic]
    fn test_bad_metrics_name() {
        let res = prometheus::register_int_gauge!(
            "ckb.chain.tip",
            "a bad metric which contains '.' in its name"
        );
        assert!(res.is_err());
        let res = prometheus::register_int_gauge!(
            "ckb-chain-tip",
            "a bad metric which contains '-' in its name"
        );
        assert!(res.is_err());
    }
}
