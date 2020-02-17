use super::{MeasureRecord, TracedTag};
use std::{
    borrow::Borrow,
    collections::{self as base, hash_map::RandomState},
    fmt,
    hash::{BuildHasher, Hash},
    ops,
    sync::Arc,
};

#[derive(Clone)]
pub struct HashSet<T, S = RandomState> {
    tag: Arc<String>,
    base: base::HashSet<T, S>,
}

impl<T, S> HashSet<T, S> {
    fn tag(&self) -> Arc<String> {
        Arc::clone(&self.tag)
    }

    fn measure(&self) {
        let entry = MeasureRecord::HashSet {
            len: self.len(),
            cap: self.capacity(),
        };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(self.tag(), entry);
    }
}

impl<T: Hash + Eq> HashSet<T, RandomState> {
    pub fn new() -> Self {
        let ret = Self {
            tag: TracedTag::current(),
            base: base::HashSet::new(),
        };
        let entry = MeasureRecord::HashSet { len: 0, cap: 0 };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(ret.tag(), entry);
        ret
    }
}
impl<T, S> Default for HashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher + Default,
{
    fn default() -> Self {
        let ret = Self {
            tag: TracedTag::current(),
            base: base::HashSet::default(),
        };
        let entry = MeasureRecord::HashSet { len: 0, cap: 0 };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(ret.tag(), entry);
        ret
    }
}

impl<T, S> From<HashSet<T, S>> for base::HashSet<T, S> {
    fn from(set: HashSet<T, S>) -> Self {
        set.base
    }
}

impl<T, S> ops::Deref for HashSet<T, S> {
    type Target = base::HashSet<T, S>;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl<T, S> ops::DerefMut for HashSet<T, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl<T, S> HashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    pub fn insert(&mut self, value: T) -> bool {
        let tmp = self.base.insert(value);
        self.measure();
        tmp
    }

    pub fn replace(&mut self, value: T) -> Option<T> {
        let tmp = self.base.replace(value);
        self.measure();
        tmp
    }

    pub fn remove<Q: ?Sized>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq,
    {
        let tmp = self.base.remove(value);
        self.measure();
        tmp
    }

    pub fn take<Q: ?Sized>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
        Q: Hash + Eq,
    {
        let tmp = self.base.take(value);
        self.measure();
        tmp
    }
}

impl<T, S> PartialEq for HashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    fn eq(&self, other: &HashSet<T, S>) -> bool {
        self.base.eq(&other.base)
    }
}

impl<T, S> Eq for HashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
}

impl<T, S> fmt::Debug for HashSet<T, S>
where
    T: Eq + Hash + fmt::Debug,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.base, f)
    }
}
