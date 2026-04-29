#![allow(dead_code)]

mod benchmarks;

use benchmarks::overall::{run_overall_rounds, setup_chain};
use std::{fs, thread, time::Duration};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(not(feature = "jemalloc"), feature = "mimalloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(feature = "ci"))]
const DEFAULT_TXS_SIZE: usize = 500;
#[cfg(feature = "ci")]
const DEFAULT_TXS_SIZE: usize = 16;

#[cfg(not(feature = "ci"))]
const DEFAULT_ROUNDS: usize = 10;
#[cfg(feature = "ci")]
const DEFAULT_ROUNDS: usize = 3;

#[derive(Clone, Copy)]
struct MemorySnapshot {
    rss_bytes: u64,
    peak_rss_bytes: u64,
    virtual_memory_bytes: u64,
}

fn parse_status_value(line: &str) -> Option<u64> {
    line.split_ascii_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value * 1024)
}

fn read_memory_snapshot() -> MemorySnapshot {
    let status = fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let mut rss_bytes = None;
    let mut peak_rss_bytes = None;
    let mut virtual_memory_bytes = None;

    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            rss_bytes = parse_status_value(line);
        } else if line.starts_with("VmHWM:") {
            peak_rss_bytes = parse_status_value(line);
        } else if line.starts_with("VmSize:") {
            virtual_memory_bytes = parse_status_value(line);
        }
    }

    MemorySnapshot {
        rss_bytes: rss_bytes.expect("VmRSS exists"),
        peak_rss_bytes: peak_rss_bytes.expect("VmHWM exists"),
        virtual_memory_bytes: virtual_memory_bytes.expect("VmSize exists"),
    }
}

fn parse_arg(index: usize, default: usize) -> usize {
    std::env::args()
        .nth(index)
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("invalid numeric argument: {value}"))
        })
        .unwrap_or(default)
}

fn allocator_name() -> &'static str {
    if cfg!(feature = "jemalloc") {
        "jemalloc"
    } else if cfg!(feature = "mimalloc") {
        "mimalloc"
    } else {
        "system"
    }
}

fn print_snapshot(label: &str, snapshot: MemorySnapshot) {
    println!(
        "{label}: rss_bytes={} peak_rss_bytes={} virtual_memory_bytes={}",
        snapshot.rss_bytes, snapshot.peak_rss_bytes, snapshot.virtual_memory_bytes
    );
}

fn main() {
    let txs_size = parse_arg(1, DEFAULT_TXS_SIZE);
    let rounds = parse_arg(2, DEFAULT_ROUNDS);

    println!("allocator={}", allocator_name());
    println!("txs_size={txs_size}");
    println!("rounds={rounds}");

    let before = read_memory_snapshot();
    let after_setup;
    let after_workload;

    {
        let (shared, chain) = setup_chain(txs_size);
        after_setup = read_memory_snapshot();
        run_overall_rounds(&shared, &chain, rounds);
        after_workload = read_memory_snapshot();
    }

    thread::sleep(Duration::from_millis(250));
    let after_drop = read_memory_snapshot();

    print_snapshot("before", before);
    print_snapshot("after_setup", after_setup);
    print_snapshot("after_workload", after_workload);
    print_snapshot("after_drop", after_drop);
}
