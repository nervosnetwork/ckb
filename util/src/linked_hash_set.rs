//! A `HashSet` wrapper that holds value in insertion order.

use linked_hash_map::{self, Keys, LinkedHashMap};
use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasher, BuildHasherDefault, Hash};
use std::iter::Extend;

type DefaultBuildHasher = BuildHasherDefault<DefaultHasher>;

/// A HashSet that holds elements in insertion order.
///
/// ## Examples
///
/// ```
/// use ckb_util::LinkedHashSet;
///
/// let mut set = LinkedHashSet::new();
/// set.insert(2);
/// set.insert(1);
/// set.insert(3);
///
/// let items: Vec<i32> = set.iter().copied().collect();
/// assert_eq!(items, [2, 1, 3]);
/// ```
pub struct LinkedHashSet<T, S = DefaultBuildHasher> {
    map: LinkedHashMap<T, (), S>,
}

pub struct Iter<'a, K: 'a> {
    iter: Keys<'a, K, ()>,
}

impl<K> Clone for Iter<'_, K> {
    fn clone(&self) -> Self {
        Iter {
            iter: self.iter.clone(),
        }
    }
}

impl<'a, K> Iterator for Iter<'a, K>
where
    K: Eq + Hash,
{
    type Item = &'a K;

    fn next(&mut self) -> Option<&'a K> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}
pub struct Difference<'a, T: 'a, S: 'a> {
    // iterator of the first set
    iter: Iter<'a, T>,
    // the second set
    other: &'a LinkedHashSet<T, S>,
}

impl<T, S> Clone for Difference<'_, T, S> {
    fn clone(&self) -> Self {
        Difference {
            iter: self.iter.clone(),
            ..*self
        }
    }
}

impl<'a, T, S> Iterator for Difference<'a, T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        loop {
            let elt = self.iter.next()?;
            if !self.other.contains(elt) {
                return Some(elt);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

impl<T: Hash + Eq> LinkedHashSet<T, DefaultBuildHasher> {
    /// Creates a linked hash set.
    ///
    /// ## Examples
    ///
    /// ```
    /// use ckb_util::LinkedHashSet;
    /// let set: LinkedHashSet<i32> = LinkedHashSet::new();
    /// ```
    pub fn new() -> LinkedHashSet<T, DefaultBuildHasher> {
        LinkedHashSet {
            map: LinkedHashMap::default(),
        }
    }
}

impl<T, S> LinkedHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    /// Returns `true` if the set contains a value.
    ///
    /// ```
    /// use ckb_util::LinkedHashSet;
    ///
    /// let mut set: LinkedHashSet<_> = LinkedHashSet::new();
    /// set.insert(1);
    /// set.insert(2);
    /// set.insert(3);
    /// assert_eq!(set.contains(&1), true);
    /// assert_eq!(set.contains(&4), false);
    /// ```
    pub fn contains(&self, value: &T) -> bool {
        self.map.contains_key(value)
    }

    /// Returns the number of elements the set can hold without reallocating.
    pub fn capacity(&self) -> usize {
        self.map.capacity()
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns `true` if the set contains no elements.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Adds a value to the set.
    ///
    /// If the set did not have this value present, true is returned.
    ///
    /// If the set did have this value present, false is returned.
    pub fn insert(&mut self, value: T) -> bool {
        self.map.insert(value, ()).is_none()
    }

    /// Gets an iterator visiting all elements in insertion order.
    ///
    /// The iterator element type is `&'a T`.
    pub fn iter(&self) -> Iter<T> {
        Iter {
            iter: self.map.keys(),
        }
    }

    /// Visits the values representing the difference, i.e., the values that are in `self` but not in `other`.
    pub fn difference<'a>(&'a self, other: &'a LinkedHashSet<T, S>) -> Difference<'a, T, S> {
        Difference {
            iter: self.iter(),
            other,
        }
    }
}

impl<T: Hash + Eq> Default for LinkedHashSet<T, DefaultBuildHasher> {
    /// Creates an empty `HashSet<T>` with the `Default` value for the hasher.
    fn default() -> LinkedHashSet<T, DefaultBuildHasher> {
        LinkedHashSet {
            map: LinkedHashMap::default(),
        }
    }
}

impl<T, S> Extend<T> for LinkedHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.map.extend(iter.into_iter().map(|k| (k, ())));
    }
}

impl<'a, T, S> IntoIterator for &'a LinkedHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T, S> IntoIterator for LinkedHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter {
            iter: self.map.into_iter(),
        }
    }
}

pub struct IntoIter<K> {
    iter: linked_hash_map::IntoIter<K, ()>,
}

impl<K> Iterator for IntoIter<K> {
    type Item = K;

    fn next(&mut self) -> Option<K> {
        self.iter.next().map(|(k, _)| k)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<K> ExactSizeIterator for IntoIter<K> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_fixed_hash::{h256, H256};

    #[test]
    fn insertion_order() {
        let tx1 = h256!("0x3b6b6ee76e80d1662911130194db2f962a28d30bd574fa792f78debaa8e3a385");
        let tx2 = h256!("0xbd15c6158328c1dfa7eaf8eec395282844d3c436c5db25bd644dd1436608fe69");
        let tx3 = h256!("0x544e23972f2b400aa8d4147240bd30d46eb0cfe8cdb436b2c8e827a4033a1c03");
        let tx4 = h256!("0xa9cc641af5fa07606c98bba6a5774379b5ba3985a2047852cf2cb946d3387b61");
        let tx5 = h256!("0x47f40d1839c3fb56bf269605593337b2dc7db1c395b30bb9568e4274df71ea24");
        let tx6 = h256!("0x1df1e5f580c6c10b858960504f14fca4d178cbb54425d021cb2361de1079b174");

        let txs = vec![tx1, tx2, tx3, tx4, tx5, tx6];

        let mut set = LinkedHashSet::default();
        set.extend(txs.iter().cloned());
        let diff: Vec<H256> = set.difference(&LinkedHashSet::default()).cloned().collect();
        assert!(txs == diff);
    }
}
