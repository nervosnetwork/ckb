use std::{collections::HashMap, sync::Arc};

use ckb_logger::debug;
use ckb_util::RwLock;
use crossbeam_channel::Sender;

mod hash_map;
mod hash_set;

pub use hash_map::HashMap as TracedHashMap;
pub use hash_set::HashSet as TracedHashSet;

pub enum MeasureRecord {
    HashMap { len: usize, cap: usize },
    HashSet { len: usize, cap: usize },
}

pub type MeasureRecordSender = Arc<RwLock<Option<Sender<(Arc<String>, MeasureRecord)>>>>;

lazy_static::lazy_static! {
    pub(crate) static ref STATISTICS: Arc<RwLock<HashMap<Arc<String>, MeasureRecord>>> = Default::default();
    pub(crate) static ref MEASURE_SENDER: MeasureRecordSender = Arc::new(RwLock::new(None));
    static ref TRACED_TAG: RwLock<Vec<&'static str>> = Default::default();
    static ref DEFAULT_TAG: Arc<String> = Arc::new("not-set".to_owned());
}

pub fn measure(tag: Arc<String>, record: MeasureRecord) {
    if crate::interval() > 0 {
        let _ignore = MEASURE_SENDER
            .read()
            .as_ref()
            .map(|sender| sender.send((tag, record)));
    }
}

pub(crate) fn track_collections() {
    let guard = (&*STATISTICS).read();
    let mut stats = guard.iter().collect::<Vec<_>>();
    stats.sort_by(|a, b| a.0.cmp(b.0));
    for (tag, measure) in stats {
        match measure {
            MeasureRecord::HashMap { len, cap } => debug!(
                "{:20} {{ tag: {:40}, len: {:8}, cap: {:8} }}",
                "HashMap", tag, len, cap
            ),
            MeasureRecord::HashSet { len, cap } => debug!(
                "{:20} {{ tag: {:40}, len: {:8}, cap: {:8} }}",
                "HashSet", tag, len, cap
            ),
        };
    }
}

pub struct TracedTag;

impl TracedTag {
    pub fn current() -> Arc<String> {
        let tmp = (&*TRACED_TAG).read().join(".");
        if tmp.is_empty() {
            Arc::clone(&DEFAULT_TAG)
        } else {
            Arc::new(tmp)
        }
    }

    pub fn push(tag: &'static str) {
        (&*TRACED_TAG).write().push(tag);
    }

    pub fn replace_last(tag: &'static str) {
        Self::pop();
        Self::push(tag);
    }

    pub fn pop() {
        (&*TRACED_TAG).write().pop();
    }
}
