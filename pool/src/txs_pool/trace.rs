use faketime::unix_time_as_millis;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::fmt;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Action {
    AddPending,
    Proposed,
    AddCommit,
    Timeout,
    AddOrphan,
    Committed,
}

#[derive(Clone, Eq, PartialEq)]
pub struct Trace {
    pub action: Action,
    pub info: String,
    pub time: u64,
}

impl Trace {
    pub fn new(action: Action, info: String, time: u64) -> Trace {
        Trace { action, info, time }
    }
}

impl fmt::Debug for Trace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Trace {{ action: {:?}, info: {}, time: {} }}",
            self.action, self.info, self.time
        )
    }
}

impl fmt::Display for Trace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}

macro_rules! define_method {
    ($name:ident, $action:expr) => {
        pub fn $name<S: ToString>(&mut self, hash: &H256, info: S) {
            self.inner.get_mut(hash).map(|v| {
                v.push(Trace::new(
                    $action,
                    info.to_string(),
                    unix_time_as_millis(),
                ))
            });
        }
    };
}

#[derive(Clone, Debug)]
pub struct TraceMap {
    inner: LruCache<H256, Vec<Trace>>,
}

impl TraceMap {
    pub fn new(capacity: usize) -> Self {
        TraceMap {
            inner: LruCache::new(capacity),
        }
    }

    pub fn add_pending<S: ToString>(&mut self, hash: &H256, info: S) {
        self.inner
            .entry(hash.clone())
            .or_insert_with(Vec::new)
            .push(Trace::new(
                Action::AddPending,
                info.to_string(),
                unix_time_as_millis(),
            ));
    }

    pub fn get(&self, hash: &H256) -> Option<&Vec<Trace>> {
        self.inner.get(hash)
    }

    define_method!(proposed, Action::Proposed);
    define_method!(add_commit, Action::AddCommit);
    define_method!(add_orphan, Action::AddOrphan);
    define_method!(timeout, Action::Timeout);
    define_method!(committed, Action::Committed);
}
