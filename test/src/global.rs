use ckb_util::Mutex;
use std::path::PathBuf;
use std::sync::atomic::AtomicU16;

pub static BINARY_PATH: std::sync::LazyLock<Mutex<PathBuf>> =
    std::sync::LazyLock::new(|| Mutex::new(PathBuf::new()));
pub static VENDOR_PATH: std::sync::LazyLock<Mutex<PathBuf>> = std::sync::LazyLock::new(|| {
    let default = ::std::env::current_dir()
        .expect("can't get current_dir")
        .join("vendor");
    Mutex::new(default)
});
pub static PORT_COUNTER: std::sync::LazyLock<AtomicU16> =
    std::sync::LazyLock::new(|| AtomicU16::new(9000));

pub fn binary() -> PathBuf {
    (*BINARY_PATH.lock()).clone()
}

pub fn vendor() -> PathBuf {
    (*VENDOR_PATH.lock()).clone()
}
