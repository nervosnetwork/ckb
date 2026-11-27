use ckb_util::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering::SeqCst};

pub static BINARY_PATH: std::sync::LazyLock<Mutex<PathBuf>> =
    std::sync::LazyLock::new(|| Mutex::new(PathBuf::new()));
pub static OBFS4PROXY_BINARY_PATH: std::sync::LazyLock<Mutex<PathBuf>> =
    std::sync::LazyLock::new(|| Mutex::new(PathBuf::new()));
pub static VENDOR_PATH: std::sync::LazyLock<Mutex<PathBuf>> = std::sync::LazyLock::new(|| {
    let default = ::std::env::current_dir()
        .expect("get current_dir")
        .join("vendor");
    Mutex::new(default)
});
pub const PORT_RANGE_START: u16 = 10_000;
pub const PORT_RANGE_LEN: u16 = 20_000;
pub const PORT_RANGE_END: u16 = PORT_RANGE_START + PORT_RANGE_LEN;

pub static PORT_AUTO_WRAP: AtomicBool = AtomicBool::new(true);

pub static PORT_COUNTER: std::sync::LazyLock<AtomicU16> = std::sync::LazyLock::new(|| {
    // Spread starting port across concurrent ckb-test invocations while staying
    // strictly below OS ephemeral port ranges (32768+ on Linux, 49152+ on macOS/Windows).
    let offset = ((std::process::id() as u32).wrapping_mul(137) % u32::from(PORT_RANGE_LEN)) as u16;
    AtomicU16::new(PORT_RANGE_START + offset)
});

pub fn set_explicit_start_port(port: u16) {
    PORT_AUTO_WRAP.store(false, SeqCst);
    PORT_COUNTER.store(port, SeqCst);
}

pub fn binary() -> PathBuf {
    (*BINARY_PATH.lock()).clone()
}

pub fn obfs4proxy_binary() -> PathBuf {
    (*OBFS4PROXY_BINARY_PATH.lock()).clone()
}

pub fn vendor() -> PathBuf {
    (*VENDOR_PATH.lock()).clone()
}
