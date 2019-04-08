#![allow(dead_code)]
use ckb_util::Mutex;
use fnv::FnvHashMap;
use lazy_static::lazy_static;
use rusqlite::Connection;
use std::time::Duration;

lazy_static! {
    pub static ref PROFILE_INFORMATION: Mutex<FnvHashMap<String, (Duration, u32)>> =
        Mutex::new(Default::default());
}

pub fn start_profile(conn: &mut Connection) {
    conn.profile(Some(profiler));
}

pub fn stop_profile(conn: &mut Connection) {
    conn.profile(None);
}

fn profiler(s: &str, d: Duration) {
    let mut profiled = PROFILE_INFORMATION.lock();
    let (ref mut total_duration, ref mut count) = profiled.entry(s.to_owned()).or_default();
    *total_duration += d;
    *count += 1;
}
