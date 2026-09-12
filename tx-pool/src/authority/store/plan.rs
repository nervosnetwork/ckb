//! A decision owns its original observations, intended owner edits and notices.
//! Tracked reads extend that decision; only Store can inspect its private state
//! while validating and committing it.
use super::{SHARDS, Store, WakePage};
use crate::{
    authority::{
        model::{DependencyKey, Entry, Error, RelationKey},
        notice::{Class, Effect},
    },
    error::Reject,
    util::compact_packed,
};
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::BlockView,
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
    time::Instant,
};

/// Every decision read is kept, including negative owner/spender observations.
/// Successful resolution keeps producer identities and point spender facts;
/// membership separately observes complete reader and descendant relations.
#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct ReadSet {
    pub(super) owners: BTreeMap<Byte32, Option<Weak<Entry>>>,
    pub(super) spenders: BTreeMap<OutPoint, Option<Byte32>>,
    pub(super) relations: BTreeMap<RelationKey, Option<Weak<()>>>,
    pub(super) peers: BTreeMap<PeerIndex, Option<Weak<()>>>,
    // Keep full-capture vectors out of ordinary sparse transaction read sets.
    pub(super) all: Option<Box<[u64; SHARDS]>>,
    pub(super) accepted: Option<Box<[u64; SHARDS]>>,
}
pub(super) fn same_weak<T>(a: &Option<Weak<T>>, b: &Option<Weak<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.ptr_eq(b),
        _ => false,
    }
}

/// Preserve the first observation of each key, rejecting a different reread.
fn merge_observations<K: Ord + Clone, V: Clone>(
    own: &mut BTreeMap<K, V>,
    incoming: &BTreeMap<K, V>,
    same: impl Fn(&V, &V) -> bool,
) -> Result<(), Error> {
    for (key, value) in incoming {
        if let Some(original) = own.get(key) {
            if !same(original, value) {
                return Err(Error::Stale);
            }
        } else {
            own.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

impl ReadSet {
    fn observed_owner(&self, hash: &Byte32) -> Option<&Option<Weak<Entry>>> {
        self.owners.get(hash)
    }
    pub(in crate::authority) fn observe_owner(
        &mut self,
        hash: &Byte32,
        entry: Option<&Arc<Entry>>,
    ) -> Result<(), Error> {
        let observed = entry.map(Arc::downgrade);
        if let Some(old) = self.owners.get(hash) {
            if !same_weak(old, &observed) {
                return Err(Error::Stale);
            }
        } else {
            self.owners.insert(compact_packed(hash), observed);
        }
        Ok(())
    }
    pub(in crate::authority) fn merge(&mut self, other: &Self) -> Result<(), Error> {
        merge_observations(&mut self.owners, &other.owners, same_weak)?;
        merge_observations(&mut self.spenders, &other.spenders, PartialEq::eq)?;
        merge_observations(&mut self.relations, &other.relations, same_weak)?;
        merge_observations(&mut self.peers, &other.peers, same_weak)?;
        for (own, incoming) in [
            (&mut self.all, &other.all),
            (&mut self.accepted, &other.accepted),
        ] {
            if let Some(incoming) = incoming {
                if own.as_ref().is_some_and(|old| old != incoming) {
                    return Err(Error::Stale);
                }
                if own.is_none() {
                    *own = Some(incoming.clone());
                }
            }
        }
        Ok(())
    }
    pub(in crate::authority) fn into_verification_reads(self) -> Self {
        Self {
            owners: self.owners,
            spenders: self.spenders,
            ..Self::default()
        }
    }
    pub(in crate::authority) fn spent(&self) -> impl Iterator<Item = (&OutPoint, &Byte32)> {
        self.spenders
            .iter()
            .filter_map(|(point, spender)| spender.as_ref().map(|hash| (point, hash)))
    }
}

#[derive(Clone)]
pub(in crate::authority) struct Edit {
    pub(in crate::authority) before: Option<Arc<Entry>>,
    pub(in crate::authority) after: Option<Arc<Entry>>,
}
impl Edit {
    pub(super) fn affects_accepted(&self) -> bool {
        self.before
            .as_ref()
            .is_some_and(|entry| entry.accepted().is_some())
            || self
                .after
                .as_ref()
                .is_some_and(|entry| entry.accepted().is_some())
    }
}

/// One decision owns its original observations, owner changes and obligations.
/// Producers can add observations, but cannot replace them or edit the outcome
/// vectors directly. A lifecycle revision stays paired with its snapshot.
#[derive(Clone)]
pub(in crate::authority) struct Plan {
    pub(super) view: u64,
    pub(super) reads: ReadSet,
    pub(super) edits: BTreeMap<Byte32, Edit>,
    pub(super) effects: Vec<Effect>,
    pub(super) class: Class,
    pub(super) snapshot: Option<Arc<Snapshot>>,
    pub(super) dry_run: bool,
    pub(super) clear_all: bool,
    pub(super) committed: Vec<(ProposalShortId, Byte32)>,
    pub(super) ban: Option<(PeerIndex, Instant)>,
    pub(super) peer_access: Option<(PeerIndex, bool)>,
    pub(in crate::authority) wake: BTreeSet<DependencyKey>,
    pub(super) wake_advance: Option<WakePage>,
}
impl Plan {
    pub(in crate::authority) fn new(view: u64, class: Class, reads: ReadSet) -> Self {
        Self {
            view,
            reads,
            edits: BTreeMap::new(),
            effects: Vec::new(),
            class,
            snapshot: None,
            dry_run: false,
            clear_all: false,
            committed: Vec::new(),
            ban: None,
            peer_access: None,
            wake: BTreeSet::new(),
            wake_advance: None,
        }
    }
    pub(in crate::authority) fn edit(
        &mut self,
        before: Option<Arc<Entry>>,
        after: Option<Arc<Entry>>,
        effect: Option<Effect>,
    ) -> Result<(), Error> {
        let hash = before
            .as_ref()
            .or(after.as_ref())
            .ok_or(Error::Fault("empty owner edit"))?
            .hash();
        if after.as_ref().is_some_and(|entry| entry.hash() != hash) {
            return Err(Error::Fault("owner hash change"));
        }
        if self.edits.contains_key(&hash) {
            return Err(Error::Fault("duplicate owner edit"));
        }
        self.reads.observe_owner(&hash, before.as_ref())?;
        // An identical immutable owner is only a read. Re-inserting its queue
        // projection could duplicate work that a worker has already selected.
        if before
            .as_ref()
            .zip(after.as_ref())
            .is_some_and(|(before, after)| Arc::ptr_eq(before, after))
        {
            self.effects.extend(effect);
            return Ok(());
        }
        self.edits
            .insert(compact_packed(&hash), Edit { before, after });
        self.effects.extend(effect);
        Ok(())
    }
    /// Notice-only outcomes have no owner edit, but still validate this Plan.
    pub(in crate::authority) fn notify(&mut self, effect: Effect) {
        self.effects.push(effect);
    }
    pub(in crate::authority) fn peer_banned(
        &mut self,
        store: &Store,
        peer: PeerIndex,
    ) -> Result<bool, Error> {
        let banned = store.peer_banned(peer);
        if self
            .peer_access
            .is_some_and(|original| original != (peer, banned))
        {
            return Err(Error::Stale);
        }
        self.peer_access = Some((peer, banned));
        Ok(banned)
    }
    pub(in crate::authority) fn ban_peer(
        &mut self,
        hash: &Byte32,
        reject: Reject,
        peer: PeerIndex,
        deadline: Instant,
    ) -> Result<(), Error> {
        if self.ban.is_some() {
            return Err(Error::Fault("duplicate peer ban"));
        }
        let effect = Effect::banned(hash, reject, peer, deadline)?;
        self.ban = Some((peer, deadline));
        self.notify(effect);
        Ok(())
    }
    /// Resetting the lifecycle and invalidating relay knowledge are one outcome.
    pub(in crate::authority) fn reset(&mut self, snapshot: Arc<Snapshot>, clear_all: bool) {
        self.snapshot = Some(snapshot);
        self.clear_all = clear_all;
        self.notify(Effect::reset());
    }
    /// Block observations and ordered committed-hash records come from the same
    /// attached blocks; source preparation cannot append one without the other.
    pub(in crate::authority) fn chain(
        &mut self,
        snapshot: Arc<Snapshot>,
        blocks: impl IntoIterator<Item = Arc<BlockView>>,
    ) {
        self.snapshot = Some(snapshot);
        let blocks: Vec<_> = blocks.into_iter().collect();
        for block in &blocks {
            for transaction in block.transactions().iter().skip(1) {
                self.committed.push((
                    compact_packed(&transaction.proposal_short_id()),
                    compact_packed(&transaction.hash()),
                ));
            }
        }
        if !blocks.is_empty() {
            self.notify(Effect::blocks(blocks));
        }
    }
    #[cfg(test)]
    pub(in crate::authority) fn committed_for_test(
        &mut self,
        records: Vec<(ProposalShortId, Byte32)>,
    ) {
        self.committed = records;
    }
    pub(in crate::authority) fn get(
        &mut self,
        store: &Store,
        hash: &Byte32,
    ) -> Result<Option<Arc<Entry>>, Error> {
        store.get(hash, &mut self.reads)
    }
    /// Graph policy reuses the first immutable observation, including absence.
    pub(in crate::authority) fn original(
        &mut self,
        store: &Store,
        hash: &Byte32,
    ) -> Result<Option<Arc<Entry>>, Error> {
        match self.reads.observed_owner(hash) {
            Some(None) => Ok(None),
            Some(Some(owner)) => owner.upgrade().map(Some).ok_or(Error::Stale),
            None => self.get(store, hash),
        }
    }
    pub(in crate::authority) fn observe_owner(
        &mut self,
        hash: &Byte32,
        entry: Option<&Arc<Entry>>,
    ) -> Result<(), Error> {
        self.reads.observe_owner(hash, entry)
    }
    pub(in crate::authority) fn spender(
        &mut self,
        store: &Store,
        point: &OutPoint,
    ) -> Result<Option<Byte32>, Error> {
        store.spender(point, &mut self.reads)
    }
    pub(in crate::authority) fn members(
        &mut self,
        store: &Store,
        key: &RelationKey,
        roles: u8,
    ) -> Result<Vec<Byte32>, Error> {
        store.members(key, roles, &mut self.reads)
    }
    pub(in crate::authority) fn peer_members(
        &mut self,
        store: &Store,
        peer: PeerIndex,
    ) -> Result<Vec<Byte32>, Error> {
        store.peer_members(peer, &mut self.reads)
    }
    pub(in crate::authority) fn capture_accepted(
        &mut self,
        store: &Store,
    ) -> Result<Vec<Arc<Entry>>, Error> {
        let (view, _, entries, reads) = store.capture(true);
        if view != self.view {
            return Err(Error::Stale);
        }
        self.reads.merge(&reads)?;
        for entry in &entries {
            self.observe_owner(&entry.hash(), Some(entry))?;
        }
        Ok(entries)
    }
    /// A refused policy keeps every premise, including peer eligibility, while
    /// dropping speculative owners and notices before constructing its rejection.
    pub(in crate::authority) fn discard_changes(self) -> Self {
        Self {
            peer_access: self.peer_access,
            dry_run: self.dry_run,
            ..Self::new(self.view, self.class, self.reads)
        }
    }
    pub(in crate::authority) fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self.effects.clear();
        self
    }
    #[cfg(any(test, feature = "internal"))]
    pub(in crate::authority) fn edits(&self) -> &BTreeMap<Byte32, Edit> {
        &self.edits
    }
    #[cfg(test)]
    pub(in crate::authority) fn effects(&self) -> &[Effect] {
        &self.effects
    }
    #[cfg(any(test, feature = "internal"))]
    pub(in crate::authority) fn silence_fixture(&mut self) {
        self.effects.clear();
    }
    pub(in crate::authority) fn advance(&mut self, page: WakePage) {
        self.wake_advance = Some(page);
    }
}
