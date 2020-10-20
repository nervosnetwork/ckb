use ckb_util::Mutex;
use lazy_static::lazy_static;
use std::path::PathBuf;
use std::sync::atomic::AtomicU16;

lazy_static! {
    pub static ref BINARY_PATH: Mutex<PathBuf> = Mutex::new(PathBuf::new());
    pub static ref VENDOR_PATH: Mutex<PathBuf> = {
        let default = ::std::env::current_dir()
            .expect("can't get current_dir")
            .join("vendor");
        Mutex::new(default)
    };
    pub static ref PORT_COUNTER: AtomicU16 = AtomicU16::new(9000);
}

pub fn binary() -> PathBuf {
    (*BINARY_PATH.lock()).clone()
}

pub fn vendor() -> PathBuf {
    (*VENDOR_PATH.lock()).clone()
}
