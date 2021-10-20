use ckb_types::core::cell::CellMeta;
use ckb_types::packed::OutPoint;
use im::{hashmap::Entry as HamtEntry, HashMap as Hamt};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct DepGroupCache {
    map: Hamt<OutPoint, (CellMeta, Vec<CellMeta>)>,
    index: Hamt<OutPoint, HashSet<OutPoint>>,
}

impl DepGroupCache {
    pub fn new() -> DepGroupCache {
        DepGroupCache {
            map: Hamt::new(),
            index: Hamt::new(),
        }
    }

    pub fn insert(&mut self, out_point: OutPoint, resolved_dep_group: (CellMeta, Vec<CellMeta>)) {
        match self.index.entry(out_point.clone()) {
            HamtEntry::Vacant(v) => {
                let mut set = HashSet::new();
                set.insert(resolved_dep_group.0.out_point.clone());
                set.extend(
                    resolved_dep_group
                        .1
                        .iter()
                        .map(|meta| meta.out_point.clone()),
                );
                v.insert(set);

                self.map.insert(out_point, resolved_dep_group);
            }
            HamtEntry::Occupied(_) => {}
        }
    }

    pub fn remove(&mut self, out_point: &OutPoint) {
        let removed_keys: Vec<_> = self
            .index
            .iter()
            .filter_map(|(k, v)| if v.contains(out_point) { Some(k) } else { None })
            .cloned()
            .collect();

        for key in removed_keys {
            self.index.remove(&key);
            self.map.remove(&key);
        }
    }

    pub fn get(&self, out_point: &OutPoint) -> Option<(CellMeta, Vec<CellMeta>)> {
        self.map.get(out_point).cloned()
    }
}
