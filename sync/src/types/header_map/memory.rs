use crate::types::{HeaderIndexView, SHRINK_THRESHOLD};
use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction},
    packed::Byte32,
    U256,
};
use ckb_util::{shrink_to_fit, LinkedHashMap, RwLock};
use std::default;

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeaderIndexViewInner {
    number: BlockNumber,
    epoch: EpochNumberWithFraction,
    timestamp: u64,
    parent_hash: Byte32,
    total_difficulty: U256,
    skip_hash: Option<Byte32>,
}

impl From<(Byte32, HeaderIndexViewInner)> for HeaderIndexView {
    fn from((hash, inner): (Byte32, HeaderIndexViewInner)) -> Self {
        let HeaderIndexViewInner {
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
            skip_hash,
        } = inner;
        Self {
            hash,
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
            skip_hash,
        }
    }
}

impl From<HeaderIndexView> for (Byte32, HeaderIndexViewInner) {
    fn from(view: HeaderIndexView) -> Self {
        let HeaderIndexView {
            hash,
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
            skip_hash,
        } = view;
        (
            hash,
            HeaderIndexViewInner {
                number,
                epoch,
                timestamp,
                parent_hash,
                total_difficulty,
                skip_hash,
            },
        )
    }
}

pub(crate) struct MemoryMap(RwLock<LinkedHashMap<Byte32, HeaderIndexViewInner>>);

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

    pub(crate) fn get_refresh(&self, key: &Byte32) -> Option<HeaderIndexView> {
        let mut guard = self.0.write();
        guard
            .get_refresh(key)
            .cloned()
            .map(|inner| (key.clone(), inner).into())
    }

    pub(crate) fn insert(&self, header: HeaderIndexView) -> Option<()> {
        let mut guard = self.0.write();
        let (key, value) = header.into();
        guard.insert(key, value).map(|_| ())
    }

    pub(crate) fn remove(&self, key: &Byte32) -> Option<HeaderIndexView> {
        let mut guard = self.0.write();
        let ret = guard.remove(key);
        shrink_to_fit!(guard, SHRINK_THRESHOLD);
        ret.map(|inner| (key.clone(), inner).into())
    }

    pub(crate) fn front_n(&self, size_limit: usize) -> Option<Vec<HeaderIndexView>> {
        let guard = self.0.read();
        let size = guard.len();
        if size > size_limit {
            let num = size - size_limit;
            Some(
                guard
                    .iter()
                    .take(num)
                    .map(|(key, value)| (key.clone(), value.clone()).into())
                    .collect(),
            )
        } else {
            None
        }
    }

    pub(crate) fn remove_batch(&self, keys: impl Iterator<Item = Byte32>) {
        let mut guard = self.0.write();
        for key in keys {
            guard.remove(&key);
        }
        shrink_to_fit!(guard, SHRINK_THRESHOLD);
    }
}
