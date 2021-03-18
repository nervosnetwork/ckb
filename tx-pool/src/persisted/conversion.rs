use ckb_types::{bytes, packed, prelude::*};
use std::collections::{HashMap, HashSet};

use crate::{component, persisted};

impl Pack<persisted::CacheEntry> for component::CacheEntry {
    fn pack(&self) -> persisted::CacheEntry {
        persisted::CacheEntry::new_builder()
            .cycles(self.cycles.pack())
            .fee(self.fee.pack())
            .build()
    }
}

impl<'r> Unpack<component::CacheEntry> for persisted::CacheEntryReader<'r> {
    fn unpack(&self) -> component::CacheEntry {
        component::CacheEntry {
            cycles: self.cycles().unpack(),
            fee: self.fee().unpack(),
        }
    }
}

impl Pack<persisted::TxEntry> for component::TxEntry {
    fn pack(&self) -> persisted::TxEntry {
        persisted::TxEntry::new_builder()
            .transaction(self.transaction.pack())
            .cycles(self.cycles.pack())
            .size(self.size.pack())
            .fee(self.fee.pack())
            .ancestors_size(self.ancestors_size.pack())
            .ancestors_fee(self.ancestors_fee.pack())
            .ancestors_cycles(self.ancestors_cycles.pack())
            .ancestors_count(self.ancestors_count.pack())
            .related_out_points(self.related_out_points.clone().pack())
            .build()
    }
}

impl<'r> Unpack<component::TxEntry> for persisted::TxEntryReader<'r> {
    fn unpack(&self) -> component::TxEntry {
        let related_out_points = self
            .related_out_points()
            .iter()
            .map(|op| op.to_entity())
            .collect();
        component::TxEntry {
            transaction: self.transaction().unpack(),
            cycles: self.cycles().unpack(),
            size: self.size().unpack(),
            fee: self.fee().unpack(),
            ancestors_size: self.ancestors_size().unpack(),
            ancestors_fee: self.ancestors_fee().unpack(),
            ancestors_cycles: self.ancestors_cycles().unpack(),
            ancestors_count: self.ancestors_count().unpack(),
            related_out_points,
        }
    }
}

impl Pack<persisted::DefectEntry> for component::DefectEntry {
    fn pack(&self) -> persisted::DefectEntry {
        let cache_entry = self
            .cache_entry
            .map(|inner| persisted::CacheEntryOpt::new_unchecked(inner.pack().as_bytes()))
            .unwrap_or_else(Default::default);
        persisted::DefectEntry::new_builder()
            .transaction(self.transaction.pack())
            .refs_count(self.refs_count.pack())
            .cache_entry(cache_entry)
            .size(self.size.pack())
            .timestamp(self.timestamp.pack())
            .build()
    }
}

impl<'r> Unpack<component::DefectEntry> for persisted::DefectEntryReader<'r> {
    fn unpack(&self) -> component::DefectEntry {
        component::DefectEntry {
            transaction: self.transaction().unpack(),
            refs_count: self.refs_count().unpack(),
            cache_entry: self.cache_entry().to_opt().map(|x| x.unpack()),
            size: self.size().unpack(),
            timestamp: self.timestamp().unpack(),
        }
    }
}

impl Pack<persisted::TxLink> for component::TxLink {
    fn pack(&self) -> persisted::TxLink {
        persisted::TxLink::new_builder()
            .parents(self.parents.clone().into_iter().pack())
            .children(self.children.clone().into_iter().pack())
            .build()
    }
}

impl<'r> Unpack<component::TxLink> for persisted::TxLinkReader<'r> {
    fn unpack(&self) -> component::TxLink {
        component::TxLink {
            parents: self.parents().to_entity().into_iter().collect(),
            children: self.children().to_entity().into_iter().collect(),
        }
    }
}

impl Pack<persisted::AncestorsScoreSortKey> for component::AncestorsScoreSortKey {
    fn pack(&self) -> persisted::AncestorsScoreSortKey {
        persisted::AncestorsScoreSortKey::new_builder()
            .fee(self.fee.pack())
            .vbytes(self.vbytes.pack())
            .id(self.id.clone())
            .ancestors_fee(self.ancestors_fee.pack())
            .ancestors_vbytes(self.ancestors_vbytes.pack())
            .ancestors_size(self.ancestors_size.pack())
            .build()
    }
}

impl<'r> Unpack<component::AncestorsScoreSortKey> for persisted::AncestorsScoreSortKeyReader<'r> {
    fn unpack(&self) -> component::AncestorsScoreSortKey {
        component::AncestorsScoreSortKey {
            fee: self.fee().unpack(),
            vbytes: self.vbytes().unpack(),
            id: self.id().to_entity(),
            ancestors_fee: self.ancestors_fee().unpack(),
            ancestors_vbytes: self.ancestors_vbytes().unpack(),
            ancestors_size: self.ancestors_size().unpack(),
        }
    }
}

impl Pack<persisted::ProposalShortIdKeyValue> for (packed::ProposalShortId, bytes::Bytes) {
    fn pack(&self) -> persisted::ProposalShortIdKeyValue {
        let (ref key, ref value) = self;
        persisted::ProposalShortIdKeyValue::new_builder()
            .key(key.to_owned())
            .value(value.pack())
            .build()
    }
}

impl Pack<persisted::ProposalShortIdKeyValueVec>
    for HashMap<packed::ProposalShortId, component::TxEntry>
{
    fn pack(&self) -> persisted::ProposalShortIdKeyValueVec {
        let items = self
            .iter()
            .map(|(key, value)| (key.to_owned(), value.pack().as_bytes()).pack());
        persisted::ProposalShortIdKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl Pack<persisted::ProposalShortIdKeyValueVec>
    for HashMap<packed::ProposalShortId, component::TxLink>
{
    fn pack(&self) -> persisted::ProposalShortIdKeyValueVec {
        let items = self
            .iter()
            .map(|(key, value)| (key.to_owned(), value.pack().as_bytes()).pack());
        persisted::ProposalShortIdKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl Pack<persisted::ProposalShortIdKeyValueVec>
    for HashMap<packed::ProposalShortId, component::DefectEntry>
{
    fn pack(&self) -> persisted::ProposalShortIdKeyValueVec {
        let items = self
            .iter()
            .map(|(key, value)| (key.to_owned(), value.pack().as_bytes()).pack());
        persisted::ProposalShortIdKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl<'r> Unpack<HashMap<packed::ProposalShortId, component::TxEntry>>
    for persisted::ProposalShortIdKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::ProposalShortId, component::TxEntry> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = persisted::TxEntryReader::new_unchecked(p.value().raw_data()).unpack();
                (k, v)
            })
            .collect()
    }
}

impl<'r> Unpack<HashMap<packed::ProposalShortId, component::TxLink>>
    for persisted::ProposalShortIdKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::ProposalShortId, component::TxLink> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = persisted::TxLinkReader::new_unchecked(p.value().raw_data()).unpack();
                (k, v)
            })
            .collect()
    }
}

impl<'r> Unpack<HashMap<packed::ProposalShortId, component::DefectEntry>>
    for persisted::ProposalShortIdKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::ProposalShortId, component::DefectEntry> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = persisted::DefectEntryReader::new_unchecked(p.value().raw_data()).unpack();
                (k, v)
            })
            .collect()
    }
}

impl Pack<persisted::SortedTxMap> for component::SortedTxMap {
    fn pack(&self) -> persisted::SortedTxMap {
        let sorted_index = persisted::AncestorsScoreSortKeyVec::new_builder()
            .set(self.sorted_index.iter().map(|v| v.pack()).collect())
            .build();
        persisted::SortedTxMap::new_builder()
            .entries(self.entries.pack())
            .sorted_index(sorted_index)
            .links(self.links.pack())
            .max_ancestors_count(self.max_ancestors_count.pack())
            .build()
    }
}

impl<'r> Unpack<component::SortedTxMap> for persisted::SortedTxMapReader<'r> {
    fn unpack(&self) -> component::SortedTxMap {
        component::SortedTxMap {
            entries: self.entries().unpack(),
            sorted_index: self.sorted_index().iter().map(|op| op.unpack()).collect(),
            links: self.links().unpack(),
            max_ancestors_count: self.max_ancestors_count().unpack(),
        }
    }
}

impl Pack<persisted::OutPointKeyValue> for (packed::OutPoint, bytes::Bytes) {
    fn pack(&self) -> persisted::OutPointKeyValue {
        let (ref key, ref value) = self;
        persisted::OutPointKeyValue::new_builder()
            .key(key.to_owned())
            .value(value.pack())
            .build()
    }
}

impl Pack<persisted::OutPointKeyValueVec>
    for HashMap<packed::OutPoint, Option<packed::ProposalShortId>>
{
    fn pack(&self) -> persisted::OutPointKeyValueVec {
        let items = self.iter().map(|(key, value)| {
            let value_opt = value
                .clone()
                .map(|inner| persisted::ProposalShortIdOpt::new_unchecked(inner.as_bytes()))
                .unwrap_or_else(Default::default);
            (key.to_owned(), value_opt.as_bytes()).pack()
        });
        persisted::OutPointKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl Pack<persisted::OutPointKeyValueVec>
    for HashMap<packed::OutPoint, Vec<packed::ProposalShortId>>
{
    fn pack(&self) -> persisted::OutPointKeyValueVec {
        let items = self
            .iter()
            .map(|(key, value)| (key.to_owned(), value.clone().pack().as_bytes()).pack());
        persisted::OutPointKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl Pack<persisted::OutPointKeyValueVec>
    for HashMap<packed::OutPoint, HashSet<packed::ProposalShortId>>
{
    fn pack(&self) -> persisted::OutPointKeyValueVec {
        let items = self.iter().map(|(key, value)| {
            (key.to_owned(), value.clone().into_iter().pack().as_bytes()).pack()
        });
        persisted::OutPointKeyValueVec::new_builder()
            .extend(items)
            .build()
    }
}

impl<'r> Unpack<HashMap<packed::OutPoint, Option<packed::ProposalShortId>>>
    for persisted::OutPointKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::OutPoint, Option<packed::ProposalShortId>> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = persisted::ProposalShortIdOptReader::new_unchecked(p.value().raw_data())
                    .to_entity()
                    .to_opt();
                (k, v)
            })
            .collect()
    }
}

impl<'r> Unpack<HashMap<packed::OutPoint, Vec<packed::ProposalShortId>>>
    for persisted::OutPointKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::OutPoint, Vec<packed::ProposalShortId>> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = packed::ProposalShortIdVecReader::new_unchecked(p.value().raw_data())
                    .to_entity()
                    .into_iter()
                    .collect();
                (k, v)
            })
            .collect()
    }
}

impl<'r> Unpack<HashMap<packed::OutPoint, HashSet<packed::ProposalShortId>>>
    for persisted::OutPointKeyValueVecReader<'r>
{
    fn unpack(&self) -> HashMap<packed::OutPoint, HashSet<packed::ProposalShortId>> {
        self.iter()
            .map(|p| {
                let k = p.key().to_entity();
                let v = packed::ProposalShortIdVecReader::new_unchecked(p.value().raw_data())
                    .to_entity()
                    .into_iter()
                    .collect();
                (k, v)
            })
            .collect()
    }
}

impl Pack<persisted::OutPointEdges>
    for component::Edges<packed::OutPoint, packed::ProposalShortId>
{
    fn pack(&self) -> persisted::OutPointEdges {
        persisted::OutPointEdges::new_builder()
            .inner(self.inner.pack())
            .outer(self.outer.pack())
            .deps(self.deps.pack())
            .build()
    }
}

impl<'r> Unpack<component::Edges<packed::OutPoint, packed::ProposalShortId>>
    for persisted::OutPointEdgesReader<'r>
{
    fn unpack(&self) -> component::Edges<packed::OutPoint, packed::ProposalShortId> {
        component::Edges {
            inner: self.inner().unpack(),
            outer: self.outer().unpack(),
            deps: self.deps().unpack(),
        }
    }
}

impl Pack<persisted::PendingQueue> for component::PendingQueue {
    fn pack(&self) -> persisted::PendingQueue {
        persisted::PendingQueue::new_builder()
            .inner(self.inner.pack())
            .build()
    }
}

impl<'r> Unpack<component::PendingQueue> for persisted::PendingQueueReader<'r> {
    fn unpack(&self) -> component::PendingQueue {
        component::PendingQueue {
            inner: self.inner().unpack(),
        }
    }
}

impl Pack<persisted::ProposedPool> for component::ProposedPool {
    fn pack(&self) -> persisted::ProposedPool {
        persisted::ProposedPool::new_builder()
            .edges(self.edges.pack())
            .inner(self.inner.pack())
            .build()
    }
}

impl<'r> Unpack<component::ProposedPool> for persisted::ProposedPoolReader<'r> {
    fn unpack(&self) -> component::ProposedPool {
        component::ProposedPool {
            edges: self.edges().unpack(),
            inner: self.inner().unpack(),
        }
    }
}

impl Pack<persisted::OrphanPool> for component::OrphanPool {
    fn pack(&self) -> persisted::OrphanPool {
        persisted::OrphanPool::new_builder()
            .vertices(self.vertices.pack())
            .edges(self.edges.pack())
            .prune_threshold(self.prune_threshold.pack())
            .build()
    }
}

impl<'r> Unpack<component::OrphanPool> for persisted::OrphanPoolReader<'r> {
    fn unpack(&self) -> component::OrphanPool {
        component::OrphanPool {
            vertices: self.vertices().unpack(),
            edges: self.edges().unpack(),
            prune_threshold: self.prune_threshold().unpack(),
        }
    }
}
