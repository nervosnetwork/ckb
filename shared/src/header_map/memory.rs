use crate::header_view::HeaderView;
use ckb_types::packed::Byte32;
use ckb_util::shrink_to_fit;
use ckb_util::LinkedHashMap;
use ckb_util::RwLock;
use std::default;

const HEADER_MAP_SHRINK_THRESHOLD: usize = 300;

pub(crate) struct MemoryMap(RwLock<LinkedHashMap<Byte32, HeaderView>>);

impl default::Default for MemoryMap {
    fn default() -> Self {
        Self(RwLock::new(default::Default::default()))
    }
}

impl MemoryMap {
    #[cfg(feature = "stats")]
    pub(crate) fn len(&self) -> usize {
        self.0.read().len()
    }

    pub(crate) fn contains_key(&self, key: &Byte32) -> bool {
        self.0.read().contains_key(key)
    }

    pub(crate) fn get_refresh(&self, key: &Byte32) -> Option<HeaderView> {
        let mut guard = self.0.write();
        guard.get_refresh(key).cloned()
    }

    pub(crate) fn insert(&self, key: Byte32, value: HeaderView) -> Option<()> {
        let mut guard = self.0.write();
        guard.insert(key, value).map(|_| ())
    }

    pub(crate) fn remove(&self, key: &Byte32) -> Option<HeaderView> {
        let mut guard = self.0.write();
        let ret = guard.remove(key);
        shrink_to_fit!(guard, HEADER_MAP_SHRINK_THRESHOLD);
        ret
    }

    pub(crate) fn front_n(&self, size_limit: usize) -> Option<Vec<HeaderView>> {
        let guard = self.0.read();
        let size = guard.len();
        if size > size_limit {
            let num = size - size_limit;
            Some(guard.values().take(num).cloned().collect())
        } else {
            None
        }
    }

    pub(crate) fn remove_batch(&self, keys: impl Iterator<Item = Byte32>) {
        let mut guard = self.0.write();
        for key in keys {
            guard.remove(&key);
        }
        shrink_to_fit!(guard, HEADER_MAP_SHRINK_THRESHOLD);
    }
}
