use std::{
    borrow::Borrow,
    collections::{
        self as base,
        hash_map::{Entry, IntoIter, Iter, IterMut, RandomState},
    },
    fmt,
    hash::{BuildHasher, Hash},
    ops,
    sync::Arc,
};

use super::{MeasureRecord, TracedTag};

#[derive(Clone)]
pub struct HashMap<K, V, S = RandomState> {
    tag: Arc<String>,
    base: base::HashMap<K, V, S>,
}

impl<K, V, S> HashMap<K, V, S> {
    fn tag(&self) -> Arc<String> {
        Arc::clone(&self.tag)
    }

    fn measure(&self) {
        let entry = MeasureRecord::HashMap {
            len: self.len(),
            cap: self.capacity(),
        };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(self.tag(), entry);
    }
}

impl<K: Hash + Eq, V> HashMap<K, V, RandomState> {
    pub fn new() -> Self {
        let ret = Self {
            tag: TracedTag::current(),
            base: base::HashMap::new(),
        };
        let entry = MeasureRecord::HashMap { len: 0, cap: 0 };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(ret.tag(), entry);
        ret
    }
}

impl<K, V, S> Default for HashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher + Default,
{
    fn default() -> Self {
        let ret = Self {
            tag: TracedTag::current(),
            base: base::HashMap::default(),
        };
        let entry = MeasureRecord::HashMap { len: 0, cap: 0 };
        (&*super::STATISTICS)
            .write()
            .unwrap()
            .insert(ret.tag(), entry);
        ret
    }
}

impl<K, V, S> From<HashMap<K, V, S>> for base::HashMap<K, V, S> {
    fn from(map: HashMap<K, V, S>) -> Self {
        map.base
    }
}

impl<K, V, S> ops::Deref for HashMap<K, V, S> {
    type Target = base::HashMap<K, V, S>;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl<K, V, S> ops::DerefMut for HashMap<K, V, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl<K, V, S> HashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> HashMap<K, V, S> {
        Self {
            tag: TracedTag::current(),
            base: base::HashMap::with_capacity_and_hasher(capacity, hash_builder),
        }
    }

    pub fn entry(&mut self, k: K) -> Entry<'_, K, V> {
        self.measure();
        self.base.entry(k)
    }

    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let tmp = self.base.insert(k, v);
        self.measure();
        tmp
    }

    pub fn remove<Q: ?Sized>(&mut self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        let tmp = self.base.remove(k);
        self.measure();
        tmp
    }
}

impl<K, V, S> PartialEq for HashMap<K, V, S>
where
    K: Eq + Hash,
    V: PartialEq,
    S: BuildHasher,
{
    fn eq(&self, other: &HashMap<K, V, S>) -> bool {
        self.base.eq(&other.base)
    }
}

impl<K, V, S> Eq for HashMap<K, V, S>
where
    K: Eq + Hash,
    V: Eq,
    S: BuildHasher,
{
}

impl<K, V, S> fmt::Debug for HashMap<K, V, S>
where
    K: Eq + Hash + fmt::Debug,
    V: fmt::Debug,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.base, f)
    }
}

impl<'a, K, V, S> IntoIterator for &'a HashMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    #[inline]
    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut HashMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;
    #[inline]
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V, S> IntoIterator for HashMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;
    #[inline]
    fn into_iter(self) -> IntoIter<K, V> {
        self.base.into_iter()
    }
}
