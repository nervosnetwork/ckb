//! Direct ingress, resolution settlement, capacity refusal and peer revocation
//! committed atomically against `AuthorityState`.

use super::{
    budget::{Account, Amount, Limits, owner_accounts},
    jobs::Resolution,
    model::{
        Entry, Error, FullReason, Phase, Premises, RemoteOrigin, Source, hash_key, proposal_key,
    },
    notice::{Batch, Class, Effect},
    state::{AuthorityState, FastHashSet, OwnerEdit},
    store::{Commit, Store},
};
use crate::{
    error::Reject, service::TxVerificationResult, util::compact_packed,
    verification::non_contextual_verify,
};
use ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL;
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::{Byte32, ProposalShortId},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn remote_source(peer: PeerIndex, cycles: Cycle) -> Result<Source, Error> {
    let duration = Duration::from_secs(
        100_u64
            .checked_mul(MAX_BLOCK_INTERVAL)
            .ok_or(Error::Fault("remote duration"))?,
    );
    let deadline = Instant::now()
        .checked_add(duration)
        .ok_or(Error::Fault("remote deadline"))?;
    Ok(Source::Remote {
        origin: RemoteOrigin { peer, deadline },
        cycles: Some(cycles),
    })
}

pub(super) fn class(source: Source) -> Class {
    if matches!(source, Source::Remote { .. }) {
        Class::Remote
    } else {
        Class::Trusted
    }
}

/// Maximum fresh transactions committed per single write-lock acquisition in a
/// network relay or proposal batch, balancing lock amortization with reorg preemption.
pub(super) const INGRESS_BATCH_CHUNK: usize = 32;

fn is_fresh_unbanned(
    state: &AuthorityState,
    now: Instant,
    peer: Option<PeerIndex>,
    hash: [u8; 32],
    seen_hashes: &mut FastHashSet<[u8; 32]>,
    seen_proposals: &mut FastHashSet<[u8; ProposalShortId::TOTAL_SIZE]>,
) -> bool {
    if peer.is_some_and(|p| state.is_peer_banned(p, now)) {
        return false;
    }
    if state.owner(&hash).is_some() || !seen_hashes.insert(hash) {
        return false;
    }
    let proposal = proposal_key(&hash);
    !state.proposals.contains_key(&proposal) && seen_proposals.insert(proposal)
}

struct ChunkQuota {
    pipeline: Amount,
    remote: Amount,
    peer: Option<(PeerIndex, Amount)>,
}

impl ChunkQuota {
    fn try_add(&mut self, state: &AuthorityState, limits: &Limits, entry: &Entry) -> bool {
        let (mut pipeline, mut remote, mut peer_used) = (self.pipeline, self.remote, self.peer);
        let fits =
            |cur: Amount, add: Amount, lim: Amount| cur.checked_add(add).filter(|v| v.fits(lim));
        for (account, charge) in owner_accounts(entry).into_iter().flatten() {
            match account {
                Account::Pipeline => match fits(pipeline, charge, limits.pipeline) {
                    Some(v) => pipeline = v,
                    None => return false,
                },
                Account::Remote => match fits(remote, charge, limits.remote) {
                    Some(v) => remote = v,
                    None => return false,
                },
                Account::Peer(peer) => {
                    let base = match peer_used {
                        Some((p, used)) if p == peer => used,
                        Some(_) => return false,
                        None => state.usage.get(Account::Peer(peer)),
                    };
                    match fits(base, charge, limits.peer) {
                        Some(v) => peer_used = Some((peer, v)),
                        None => return false,
                    }
                }
                Account::Accepted | Account::History => return false,
            }
        }
        (self.pipeline, self.remote, self.peer) = (pipeline, remote, peer_used);
        true
    }
}

/// Fast-path chunked ingress for fresh, valid transactions in a network relay
/// or proposal batch.
///
/// Pre-screens the fresh, unbanned prefix under one read lock, runs
/// non-contextual consensus checks and `Entry` construction off-lock, then
/// installs the longest within-budget prefix under one write lock.
pub(super) fn commit_ingress_chunk(
    store: &Store,
    items: &[(Arc<TransactionView>, Source)],
) -> Result<usize, Error> {
    let Some((_, first_source)) = items.first() else {
        return Ok(0);
    };
    let effect_class = class(*first_source);
    let (snapshot, fresh_prefix) = {
        let state = store.state_read();
        let now = Instant::now();
        let mut seen_hashes = FastHashSet::default();
        let mut seen_proposals = FastHashSet::default();
        let count = items
            .iter()
            .take_while(|(tx, source)| {
                let hash = hash_key(tx.hash_ref());
                !(matches!(source, Source::Remote { .. })
                    && store.is_budget_cooled_down(hash, hash_key(&tx.witness_hash()), now))
                    && is_fresh_unbanned(
                        &state,
                        now,
                        source.residency_peer(),
                        hash,
                        &mut seen_hashes,
                        &mut seen_proposals,
                    )
            })
            .count();
        (Arc::clone(&state.snapshot), count)
    };
    let Some(fresh_items) = items.get(..fresh_prefix).filter(|slice| !slice.is_empty()) else {
        return Ok(0);
    };
    let max_cycles = snapshot.consensus().max_block_cycles();
    let mut prepared = Vec::with_capacity(fresh_items.len());
    for (transaction, source) in fresh_items {
        if source
            .declared_cycles()
            .is_some_and(|cycles| cycles > max_cycles)
            || non_contextual_verify(snapshot.consensus(), transaction).is_err()
        {
            break;
        }
        let Ok(entry) = Entry::try_new(
            Arc::clone(transaction),
            store.next_arrival()?,
            *source,
            Phase::Resolve,
        ) else {
            break;
        };
        if store.budget.limits.resolved_fits(&entry).is_err() {
            break;
        }
        prepared.push((entry.hash_bytes(), entry));
    }
    if prepared.is_empty() {
        return Ok(0);
    }
    let (committed, _) = store.commit_with(|tx| {
        let mut seen_hashes = FastHashSet::default();
        let mut seen_proposals = FastHashSet::default();
        let mut quota = ChunkQuota {
            pipeline: tx.state.usage.pipeline,
            remote: tx.state.usage.remote,
            peer: None,
        };
        let mut edits = Vec::with_capacity(prepared.len());
        for (hash, entry) in prepared {
            if (matches!(entry.source, Source::Remote { .. })
                && tx.is_budget_cooled_down(hash, hash_key(&entry.transaction.witness_hash())))
                || !is_fresh_unbanned(
                    tx.state,
                    tx.now,
                    entry.preaccepted_peer(),
                    hash,
                    &mut seen_hashes,
                    &mut seen_proposals,
                )
                || !quota.try_add(tx.state, tx.limits, &entry)
            {
                break;
            }
            edits.push(OwnerEdit::insert(hash, entry));
        }
        let count = edits.len();
        if count == 1 {
            if let Some(edit) = edits.into_iter().next() {
                tx.commit_single(edit.hash, edit.before, edit.after, Vec::new(), effect_class)?;
            }
        } else if count > 1 {
            tx.commit_edits(edits, Vec::new(), Vec::new(), effect_class)?;
        }
        Ok(count)
    })?;
    Ok(committed)
}

/// Validate a received transaction off-lock, then retain, acknowledge or
/// promote its live owner atomically under one short write guard.
pub(super) fn commit_ingress(
    store: &Store,
    transaction: Arc<TransactionView>,
    source: Source,
) -> Result<Option<Arc<Batch>>, Error> {
    let (_, snapshot) = store.snapshot();
    let max_cycles = snapshot.consensus().max_block_cycles();
    let validity = if source
        .declared_cycles()
        .is_some_and(|cycles| cycles > max_cycles)
    {
        Err(Reject::Malformed(
            "remote declared cycles".into(),
            format!("declared cycles exceed consensus maximum {max_cycles}"),
        ))
    } else {
        non_contextual_verify(snapshot.consensus(), &transaction)
    };
    let hash = transaction.hash();
    let hash_bytes = hash_key(&hash);
    let effect_class = class(source);

    let (_, batch) = store.commit_with(|tx| {
        if let Source::Remote { origin, .. } = source
            && (tx.state.is_peer_banned(origin.peer, tx.now)
                || if tx.is_budget_cooled_down(hash_bytes, hash_key(&transaction.witness_hash())) {
                    crate::metrics::record_verify_budget_cooldown_hit();
                    true
                } else {
                    false
                })
        {
            return tx.commit_effects(
                vec![Effect::relay(TxVerificationResult::Reject {
                    tx_hash: compact_packed(&hash),
                })],
                effect_class,
            );
        }
        if let Err(reject) = validity {
            return commit_rejection_locked(tx, None, &hash, source, reject);
        }
        let current = tx.state.owner(&hash_bytes).cloned();
        let (next_source, phase) = match (current.as_deref(), source) {
            (Some(old), Source::Remote { origin, .. }) => {
                let tx_hash = compact_packed(&hash);
                let result = if old.accepted().is_some() {
                    TxVerificationResult::Ok {
                        original_peer: Some(origin.peer),
                        tx_hash,
                    }
                } else {
                    TxVerificationResult::Reject { tx_hash }
                };
                return tx.commit_effects(vec![Effect::relay(result)], effect_class);
            }
            (Some(old), _) if old.accepted().is_some() => return Ok(()),
            (Some(old), Source::Proposal { .. }) => {
                if matches!(old.source, Source::Recovery | Source::Local) {
                    return Ok(());
                }
                let same_witness = old.transaction.witness_hash() == transaction.witness_hash();
                if same_witness && matches!(old.source, Source::Proposal { .. }) {
                    return Ok(());
                }
                let phase = match &old.phase {
                    Phase::Verify(resolved) if same_witness && resolved.view == tx.state.view => {
                        Phase::Verify(Arc::clone(resolved))
                    }
                    _ => Phase::Resolve,
                };
                (old.source.promote_remote(), phase)
            }
            (None, Source::Proposal { .. }) => (Source::Proposal { remote: None }, Phase::Resolve),
            (_, source) => (source, Phase::Resolve),
        };
        let arrival = match &current {
            Some(old) => old.arrival,
            None => store.next_arrival()?,
        };
        let after = Entry::try_new(transaction, arrival, next_source, phase)?;
        tx.limits.resolved_fits(&after)?;
        tx.commit_single(hash_bytes, current, Some(after), Vec::new(), effect_class)
    })?;
    Ok(batch)
}

/// Commit a background worker's resolution outcome (Verify, Waiting, or Rejected).
pub(super) fn commit_resolution(
    store: &Store,
    view: u64,
    before: &Arc<Entry>,
    result: &Resolution,
) -> Result<Option<Arc<Batch>>, Error> {
    let hash = before.hash();
    let hash_bytes = before.hash_bytes();
    let (_, batch) = store.commit_with(|tx| {
        if !tx
            .state
            .owner(&hash_bytes)
            .is_some_and(|entry| Arc::ptr_eq(entry, before))
        {
            return Err(Error::Stale);
        }
        match result {
            Resolution::Rejected(reject, premises) => {
                tx.state.validate_premises(view, premises)?;
                commit_rejection_locked(
                    tx,
                    Some(Arc::clone(before)),
                    &hash,
                    before.source,
                    reject.clone(),
                )
            }
            Resolution::Ready(resolved) => {
                tx.state.validate_premises(view, &resolved.premises)?;
                let after = before.try_with_phase(Phase::Verify(Arc::clone(resolved)))?;
                tx.commit_single(
                    hash_bytes,
                    Some(Arc::clone(before)),
                    Some(after),
                    Vec::new(),
                    class(before.source),
                )
            }
            Resolution::Waiting(keys, premises) => {
                tx.state.validate_premises(view, premises)?;
                let after = before.try_with_phase(Phase::Waiting(keys.clone()))?;
                let effects: Vec<Effect> = Effect::waiting(&after).into_iter().collect();
                tx.commit_single(
                    hash_bytes,
                    Some(Arc::clone(before)),
                    Some(after),
                    effects,
                    class(before.source),
                )
            }
        }
    })?;
    Ok(batch)
}

/// Record a capacity refusal without removing any existing owner.
pub(super) fn commit_capacity_refusal(
    store: &Store,
    hash: &Byte32,
    source: Source,
    reason: FullReason,
) -> Result<Option<Arc<Batch>>, Error> {
    let (_, batch) = store.commit_with(|tx| {
        let effect = Effect::capacity_refused(hash, reason, source, &tx.state.usage, tx.limits);
        tx.commit_effects(vec![effect], class(source))
    })?;
    Ok(batch)
}

/// Validate premises and commit a rejection (retiring `before` if present, or
/// revoking the entire preaccepted peer cohort on a remote malformed tx).
pub(super) fn commit_rejection(
    store: &Store,
    view: u64,
    premises: &Premises,
    before: Option<Arc<Entry>>,
    hash: &Byte32,
    source: Source,
    reject: Reject,
) -> Result<Option<Arc<Batch>>, Error> {
    let hash_bytes = hash_key(hash);
    let cooldown_witness = (matches!(reject, Reject::ExcessiveVerifyTime)
        && matches!(source, Source::Remote { .. }))
    .then(|| {
        before
            .as_ref()
            .map(|entry| hash_key(&entry.transaction.witness_hash()))
    })
    .flatten();
    let (_, batch) = store.commit_with(|tx| {
        tx.state.validate_premises(view, premises)?;
        if let Some(expected) = &before
            && !tx
                .state
                .owner(&hash_bytes)
                .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            return Err(Error::Stale);
        }
        commit_rejection_locked(tx, before, hash, source, reject)?;
        // Record remote CPU-budget cooldown only after view/premises/owner
        // validation and rejection commit all succeed in the same write lock,
        // so stale verification results from a previous view can never pollute
        // the new view's cooldown table.
        if let Some(witness_hash) = cooldown_witness {
            tx.record_budget_cooldown(hash_bytes, witness_hash);
        }
        Ok(())
    })?;
    Ok(batch)
}

/// Shared locked rejection helper used by ingress, resolution, worker failure
/// and membership admission rejections.
pub(super) fn commit_rejection_locked(
    tx: &mut Commit<'_>,
    before: Option<Arc<Entry>>,
    hash: &Byte32,
    source: Source,
    reject: Reject,
) -> Result<(), Error> {
    let effect_class = class(source);
    if reject.is_malformed_tx()
        && let Source::Remote {
            origin,
            cycles: Some(_),
        } = source
    {
        let peer = origin.peer;
        let peer_hashes: Vec<[u8; 32]> = tx
            .state
            .peer_members
            .get(&peer)
            .map(|members| members.iter().copied().collect())
            .unwrap_or_default();
        let mut edits = Vec::with_capacity(peer_hashes.len());
        for member_hash in peer_hashes {
            let entry = tx
                .state
                .owner(&member_hash)
                .filter(|entry| entry.preaccepted_peer() == Some(peer))
                .cloned()
                .ok_or(Error::Fault("peer projection"))?;
            edits.push(OwnerEdit::remove(member_hash, entry));
        }
        let deadline = tx
            .now
            .checked_add(Duration::from_secs(
                crate::constants::MALFORMED_TX_BAN_SECONDS,
            ))
            .ok_or(Error::Fault("ban deadline"))?;
        let effect = Effect::banned(hash, reject, peer, deadline)?;
        tx.commit_edits(edits, Vec::new(), vec![effect], effect_class)?;
        tx.apply_ban(peer, deadline);
        return Ok(());
    }

    let effect = Effect::candidate_rejected(
        hash,
        reject,
        source,
        before.as_deref(),
        &tx.state.usage,
        tx.limits,
    )?;
    match before {
        Some(before) => tx.commit_single(
            before.hash_bytes(),
            Some(before),
            None,
            vec![effect],
            effect_class,
        ),
        None => tx.commit_effects(vec![effect], effect_class),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::tests::common::*;
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, TransactionBuilder},
        packed::{CellInput, CellOutput, OutPoint},
        prelude::*,
    };

    fn valid_tx(seed: u8) -> Arc<TransactionView> {
        let output = CellOutput::new_builder()
            .capacity(Capacity::bytes(100).unwrap())
            .build();
        Arc::new(
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(Byte32::new([seed; 32]), 0), 0))
                .output(output)
                .output_data(Bytes::new().pack())
                .build(),
        )
    }

    #[test]
    fn chunk_ingress_commits_fresh_prefix_and_prescreens_duplicates_and_bans() {
        let store = store();
        let peer: PeerIndex = 9.into();
        let source = remote_source(peer, 1_000).unwrap();
        let tx1 = valid_tx(11);
        let tx2 = valid_tx(12);
        let tx3 = valid_tx(13);

        // Fresh prefix stops before an in-chunk duplicate without skipping order.
        let batch = vec![
            (Arc::clone(&tx1), source),
            (Arc::clone(&tx2), source),
            (Arc::clone(&tx1), source),
            (Arc::clone(&tx3), source),
        ];
        assert_eq!(commit_ingress_chunk(&store, &batch).unwrap(), 2);
        assert!(store.point(&tx1.hash()).1.is_some());
        assert!(store.point(&tx2.hash()).1.is_some());
        assert!(store.point(&tx3.hash()).1.is_none());

        // Already-owned leading item is filtered under the read lock without
        // burning arrival IDs or preparing the trailing items.
        let before_arrival = store.next_arrival().unwrap();
        assert_eq!(
            commit_ingress_chunk(
                &store,
                &[(Arc::clone(&tx1), source), (Arc::clone(&tx3), source)]
            )
            .unwrap(),
            0
        );
        assert_eq!(store.next_arrival().unwrap(), before_arrival + 1);

        // Banned peer leading item is also pre-screened before off-lock work.
        let deadline = Instant::now() + Duration::from_secs(60);
        store
            .commit_with(|tx| {
                tx.apply_ban(peer, deadline);
                Ok(())
            })
            .unwrap();
        let before_ban_arrival = store.next_arrival().unwrap();
        assert_eq!(
            commit_ingress_chunk(&store, &[(Arc::clone(&tx3), source)]).unwrap(),
            0
        );
        assert_eq!(store.next_arrival().unwrap(), before_ban_arrival + 1);
    }

    #[test]
    fn budget_cooldown_sheds_remote_replay_only_and_clears_on_reorg() {
        let store = store();
        let peer: PeerIndex = 12.into();
        let source = remote_source(peer, 1_000).unwrap();
        let tx_a = valid_tx(21);
        let tx_a_modified_witness = Arc::new(
            tx_a.as_ref()
                .clone()
                .as_advanced_builder()
                .witness(Bytes::from_static(b"re-signed").pack())
                .build(),
        );
        assert_eq!(tx_a.hash(), tx_a_modified_witness.hash());
        assert_ne!(tx_a.witness_hash(), tx_a_modified_witness.witness_hash());

        store.record_budget_cooldown(hash_key(tx_a.hash_ref()), hash_key(&tx_a.witness_hash()));

        // Chunk and single remote ingress both shed exact (tx_hash, witness_hash) replay.
        assert_eq!(
            commit_ingress_chunk(&store, &[(Arc::clone(&tx_a), source)]).unwrap(),
            0
        );
        let batch = commit_ingress(&store, Arc::clone(&tx_a), source)
            .unwrap()
            .expect("cooldown refusal emits relay reject batch");
        assert!(store.point(tx_a.hash_ref()).1.is_none());
        drop(batch);

        // Changed witness is not blocked by the old witness's cooldown.
        assert_eq!(
            commit_ingress_chunk(&store, &[(Arc::clone(&tx_a_modified_witness), source)]).unwrap(),
            1
        );
        assert!(store.point(tx_a.hash_ref()).1.is_some());

        // Proposal ingress bypasses remote cooldown even for the original witness.
        let tx_b = valid_tx(22);
        store.record_budget_cooldown(hash_key(tx_b.hash_ref()), hash_key(&tx_b.witness_hash()));
        assert_eq!(
            commit_ingress_chunk(
                &store,
                &[(Arc::clone(&tx_b), Source::Proposal { remote: None })]
            )
            .unwrap(),
            1
        );
        assert!(store.point(tx_b.hash_ref()).1.is_some());

        // Chain reorg clears the in-memory cooldown so a later remote submission enters normally.
        let tx_c = valid_tx(23);
        store.record_budget_cooldown(hash_key(tx_c.hash_ref()), hash_key(&tx_c.witness_hash()));
        assert_eq!(
            commit_ingress_chunk(&store, &[(Arc::clone(&tx_c), source)]).unwrap(),
            0
        );
        store.commit_reorg(|_| Ok(())).unwrap();
        assert_eq!(
            commit_ingress_chunk(&store, &[(Arc::clone(&tx_c), source)]).unwrap(),
            1
        );
        assert!(store.point(tx_c.hash_ref()).1.is_some());
    }
}
