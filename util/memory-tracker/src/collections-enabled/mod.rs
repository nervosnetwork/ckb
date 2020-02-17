use ckb_logger::trace;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[path = "hash_map.rs"]
mod hash_map;
#[path = "hash_set.rs"]
mod hash_set;

pub use hash_map::HashMap as TracedHashMap;
pub use hash_set::HashSet as TracedHashSet;

#[derive(Debug, Clone, Copy)]
pub(crate) enum MeasureRecord {
    HashMap { len: usize, cap: usize },
    HashSet { len: usize, cap: usize },
}

lazy_static::lazy_static! {
    static ref STATISTICS: Arc<RwLock<HashMap<Arc<String>, MeasureRecord>>> = Default::default();
    static ref TRACED_TAG: RwLock<Vec<&'static str>> = Default::default();
}

pub(crate) fn track_collections() {
    let tmp = (&*STATISTICS).read().unwrap();
    let mut stats = tmp.iter().collect::<Vec<_>>();
    stats.sort_by(|a, b| a.0.cmp(b.0));
    for (tag, measure) in stats {
        match measure {
            MeasureRecord::HashMap { len, cap } => trace!(
                "{:20} {{ tag: {:40}, len: {:8}, cap: {:8} }}",
                "HashMap",
                tag,
                len,
                cap
            ),
            MeasureRecord::HashSet { len, cap } => trace!(
                "{:20} {{ tag: {:40}, len: {:8}, cap: {:8} }}",
                "HashSet",
                tag,
                len,
                cap
            ),
        };
    }
}

pub struct TracedTag;

impl TracedTag {
    pub fn current() -> Arc<String> {
        let tmp = (&*TRACED_TAG).read().unwrap().join(".");
        let tag = if tmp.is_empty() {
            "not-set".to_owned()
        } else {
            tmp
        };
        Arc::new(tag)
    }

    pub fn push(tag: &'static str) {
        (&*TRACED_TAG).write().unwrap().push(tag);
    }

    pub fn replace_last(tag: &'static str) {
        Self::pop();
        Self::push(tag);
    }

    pub fn pop() {
        (&*TRACED_TAG).write().unwrap().pop();
    }
}
