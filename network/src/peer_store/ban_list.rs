use rusqlite::{Connection, NO_PARAMS};
use fnv::{FnvHashMap, FnvHashSet};

pub struct BanList {
    banned_ips: FnvHashMap<Ip, u32>
}

impl BanList {
    pub fn load(&conn: Connection) -> Self {
        BanList{
            banned_ips: Default::default(),
        }
    }

    pub fn ban(&mut self, &conn: Connection, ip: u32, ban_timeout: u32) {
    }

    pub fn is_banned(&self, &conn: Connection, ip: u32) -> bool {
        false
    }

    fn clear_expires(&mut self, &conn: Connection) {
    }
}

