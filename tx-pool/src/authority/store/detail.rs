//! Pending rank projections use the ancestry already proved for admission.
//! Repricing invalidates the proof; a read can reconstruct it from one owner cut.
use super::*;
use crate::{
    authority::{
        membership::{self, Aggregate},
        query::score,
    },
    component::sort_key::TransactionPriority,
};
use ckb_types::core::tx_pool::PoolTxDetailInfo;

type Priority = TransactionPriority<[u8; 32]>;

#[derive(Default)]
pub(super) struct Ranks {
    generation: u64,
    longest: usize,
    pending: Vec<Priority>,
}

impl Ranks {
    pub(super) fn empty(generation: u64) -> Self {
        Self {
            generation,
            ..Self::default()
        }
    }

    pub(super) fn invalidate(&mut self) {
        *self = Self::empty(u64::MAX);
    }

    fn current(&self, generation: u64, limit: usize) -> bool {
        generation != u64::MAX && self.generation == generation && self.longest <= limit
    }

    pub(super) fn update(
        &mut self,
        generation: u64,
        hash: &Byte32,
        edit: &Edit,
        snapshot: &Snapshot,
        ancestors: Option<Aggregate>,
    ) {
        if !self.current(generation, usize::MAX) {
            self.invalidate();
            return;
        }
        if let Some(value) = edit.before.as_deref().and_then(Entry::accepted)
            && value.status(snapshot) != Status::Proposed
            && !self.remove(hash)
        {
            return;
        }
        if let Some(owner) = &edit.after
            && let Some(value) = owner.accepted()
        {
            let Some(sum) =
                ancestors.or_else(|| value.parents.is_empty().then(|| Aggregate::one(value)))
            else {
                self.invalidate();
                return;
            };
            self.longest = self.longest.max(sum.count);
            if value.status(snapshot) != Status::Proposed {
                let Ok(score) = score(value, sum) else {
                    self.invalidate();
                    return;
                };
                let priority = TransactionPriority {
                    score,
                    arrival: owner.arrival,
                    hash: owner.transaction.hash_ref().unpack(),
                };
                let position = self.pending.partition_point(|before| before < &priority);
                self.pending.insert(position, priority);
            }
        }
    }

    /// Retire the priority for a departure or a promotion. A missing member
    /// invalidates the run, so an owner replacement must stop updating it.
    pub(super) fn remove(&mut self, hash: &Byte32) -> bool {
        let Some(position) = self
            .pending
            .iter()
            .position(|p| p.hash.as_slice() == hash.as_slice())
        else {
            self.invalidate();
            return false;
        };
        self.pending.remove(position);
        if self.pending.capacity() > 8 && self.pending.len() <= self.pending.capacity() / 4 {
            self.pending.shrink_to_fit();
        }
        true
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "The partition point is within the sorted run."
    )]
    fn ahead(&self, target: &Priority) -> usize {
        self.pending.len() - self.pending.partition_point(|entry| entry <= target)
    }
}

impl Store {
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Ranks and closure lengths are bounded by the accepted population."
    )]
    pub(in crate::authority) fn ranked_detail(
        &self,
        hash: &Byte32,
        limit: usize,
    ) -> Result<PoolTxDetailInfo, Error> {
        let view = self.view.read();
        let guards = self.shards.read_all();
        let generation = self.rank_generation.load(Ordering::SeqCst);
        let mut family = self.descendants_at(&guards, hash)?.into_iter();
        let Some(owner) = family.next() else {
            return Ok(PoolTxDetailInfo::with_unknown());
        };
        let descendants_count = family.len();
        drop(family);
        let value = owner.accepted().ok_or(Error::Stale)?;
        let status = value.status(&view.snapshot);
        let mut ancestors = Aggregate::default();
        membership::walk_ancestors(
            &owner,
            limit,
            |parent| {
                guards
                    .at(self.owner_shard(parent.as_slice()))
                    .owners
                    .get(parent.as_slice())
                    .map(Arc::as_ref)
            },
            |value| {
                ancestors = ancestors.add(Aggregate::one(value))?;
                Ok(())
            },
        )?;
        let target = Priority {
            score: score(value, ancestors)?,
            arrival: owner.arrival,
            hash: hash.unpack(),
        };
        let proposed_count = guards.iter().map(|shard| shard.proposed).sum::<usize>();
        let pending_count = self.budget.accepted_usage().items - proposed_count;
        let rank_in_pending = if guards
            .iter()
            .all(|shard| shard.ranks.current(generation, limit))
        {
            if status == Status::Proposed {
                0
            } else {
                1 + guards
                    .iter()
                    .map(|shard| shard.ranks.ahead(&target))
                    .sum::<usize>()
            }
        } else {
            let owners: Vec<_> = ShardIndex::all()
                .flat_map(|index| {
                    guards
                        .at(index)
                        .owners
                        .values()
                        .filter(|entry| entry.accepted().is_some())
                        .map(move |entry| (index, Arc::clone(entry)))
                })
                .collect();
            let revisions = guards.map(|shard| shard.accepted_revision);
            let (revision, snapshot) = (view.revision, Arc::clone(&view.snapshot));
            drop(guards);
            drop(view);
            let ranks = Self::rebuild_ranks(&owners, &snapshot, generation, limit)?;
            let ahead = ranks
                .iter()
                .map(|shard| shard.ahead(&target))
                .sum::<usize>();
            self.install_ranks(revision, generation, &revisions, ranks);
            if status == Status::Proposed {
                0
            } else {
                1 + ahead
            }
        };
        Ok(PoolTxDetailInfo {
            timestamp: value.timestamp,
            entry_status: match status {
                Status::Pending => "pending",
                Status::Gap => "gap",
                Status::Proposed => "proposed",
            }
            .to_owned(),
            rank_in_pending,
            pending_count,
            proposed_count,
            descendants_count,
            ancestors_count: ancestors.count - 1,
            score_sortkey: target.score.into(),
        })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Traversal ordinals refer to this same immutable owner slice."
    )]
    fn rebuild_ranks(
        owners: &[(ShardIndex, Arc<Entry>)],
        snapshot: &Snapshot,
        generation: u64,
        limit: usize,
    ) -> Result<Shards<Ranks>, Error> {
        let mut ranks = Shards::new(|| Ranks::empty(generation));
        let entries: Vec<_> = owners.iter().map(|(_, entry)| entry.as_ref()).collect();
        membership::visit_ancestors(&entries, limit, |root, _, sum| {
            let (index, owner) = &owners[root];
            let value = owner.accepted().ok_or(Error::Stale)?;
            let group = ranks.at_mut(*index);
            group.longest = group.longest.max(sum.count);
            if value.status(snapshot) != Status::Proposed {
                group.pending.push(TransactionPriority {
                    score: score(value, sum)?,
                    arrival: owner.arrival,
                    hash: owner.transaction.hash_ref().unpack(),
                });
            }
            Ok(())
        })?;
        for index in ShardIndex::all() {
            ranks.at_mut(index).pending.sort_unstable();
        }
        Ok(ranks)
    }

    pub(super) fn install_ranks(
        &self,
        revision: u64,
        generation: u64,
        revisions: &Shards<u64>,
        ranks: Shards<Ranks>,
    ) {
        let view = self.view.read();
        if view.revision != revision {
            return;
        }
        for (index, group) in ShardIndex::all().zip(ranks) {
            let mut shard = self.shards.at(index).write();
            if shard.accepted_revision == *revisions.at(index)
                && self.rank_generation.load(Ordering::SeqCst) == generation
            {
                let old = std::mem::replace(&mut shard.ranks, group);
                drop(shard);
                drop(old);
            }
        }
    }
}
