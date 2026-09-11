//! Source promotion and current-owner rejection plans.
use super::{
    model::{Entry, Error, Phase, Source},
    notice::{Class, Effect, bounded_ban_reason},
    store::{Plan, ReadSet, Store},
};
use crate::{
    error::Reject, service::TxVerificationResult, util::compact_packed,
    verification::non_contextual_verify,
};
use ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL;
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::Byte32,
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
        peer,
        deadline,
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

pub(super) fn prepare(
    store: &Store,
    transaction: Arc<TransactionView>,
    source: Source,
) -> Result<Plan, Error> {
    let (view, snapshot) = store.snapshot();
    let mut reads = ReadSet::default();
    let current = store.get(&transaction.hash(), &mut reads)?;
    let peer_access = match source {
        Source::Remote { peer, .. } => Some((peer, store.peer_banned(peer))),
        _ => None,
    };
    if let Some((_, true)) = peer_access {
        let mut plan = Plan::new(view, class(source));
        plan.reads = reads;
        plan.peer_access = peer_access;
        plan.effects.push(Effect {
            relay: Some(TxVerificationResult::Reject {
                tx_hash: compact_packed(&transaction.hash()),
            }),
            ..Effect::default()
        });
        return Ok(plan);
    }
    let validity = if source
        .declared_cycles()
        .is_some_and(|cycles| cycles > snapshot.consensus().max_block_cycles())
    {
        Err(Reject::Malformed(
            "remote declared cycles".into(),
            format!(
                "declared cycles exceed consensus maximum {}",
                snapshot.consensus().max_block_cycles()
            ),
        ))
    } else {
        non_contextual_verify(snapshot.consensus(), &transaction)
    };
    if let Err(reject) = validity {
        let mut plan = rejection(
            store,
            view,
            None,
            &transaction.hash(),
            source,
            reject,
            reads,
        )?;
        plan.peer_access = peer_access;
        return Ok(plan);
    }
    let mut plan = Plan::new(view, class(source));
    plan.peer_access = peer_access;
    plan.reads = reads;
    match (&current, source) {
        (Some(old), Source::Remote { peer, .. }) => {
            plan.effects.push(Effect {
                relay: Some(if old.accepted().is_some() {
                    TxVerificationResult::Ok {
                        original_peer: Some(peer),
                        tx_hash: compact_packed(&transaction.hash()),
                    }
                } else {
                    TxVerificationResult::Reject {
                        tx_hash: compact_packed(&transaction.hash()),
                    }
                }),
                ..Effect::default()
            });
            return Ok(plan);
        }
        (Some(old), _) if old.accepted().is_some() => {
            return Ok(plan);
        }
        (Some(old), Source::Proposal { .. })
            if matches!(old.source, Source::Recovery | Source::Local) =>
        {
            return Ok(plan);
        }
        (Some(old), Source::Proposal { .. })
            if old.transaction.witness_hash() == transaction.witness_hash()
                && matches!(old.source, Source::Proposal { .. }) =>
        {
            return Ok(plan);
        }
        _ => {}
    }
    let source = if matches!(source, Source::Proposal { .. }) {
        Source::Proposal {
            remote: current.as_ref().and_then(|old| match old.source {
                Source::Remote { peer, deadline, .. } => Some((peer, deadline)),
                Source::Proposal { remote } => remote,
                Source::Recovery | Source::Local => None,
            }),
        }
    } else {
        source
    };
    let arrival = match &current {
        Some(old) => old.arrival,
        None => store.next_arrival()?,
    };
    let phase = match current.as_deref() {
        Some(old)
            if matches!(source, Source::Proposal { .. })
                && old.transaction.witness_hash() == transaction.witness_hash() =>
        {
            // Resolution is source-independent once successful. Verification
            // still validates its original view/reads and uses the new policy.
            match &old.phase {
                Phase::Verify(resolved) if resolved.view == view => {
                    Phase::Verify(Arc::clone(resolved))
                }
                _ => Phase::Resolve,
            }
        }
        _ => Phase::Resolve,
    };
    let after = Arc::new(Entry {
        transaction,
        arrival,
        source,
        phase,
    });
    store.budget.limits.resolved_fits(&after)?;
    plan.edit(current, Some(after))?;
    Ok(plan)
}

/// A remote malformed result revokes exactly the current preaccepted cohort.
/// The original culprit read prevents an old worker from banning after promotion.
pub(super) fn rejection(
    store: &Store,
    view: u64,
    before: Option<Arc<Entry>>,
    hash: &Byte32,
    source: Source,
    reject: Reject,
    reads: ReadSet,
) -> Result<Plan, Error> {
    let mut plan = Plan::new(view, class(source));
    plan.reads = reads;
    if let Some(before) = &before {
        plan.reads.owner(hash, Some(before))?;
    }
    if reject.is_malformed_tx()
        && let Source::Remote {
            peer,
            cycles: Some(_),
            ..
        } = source
    {
        let hashes = store.peer_members(peer, &mut plan.reads)?;
        for hash in hashes {
            let entry = store.get(&hash, &mut plan.reads)?.ok_or(Error::Stale)?;
            if !entry.preaccepted() || entry.source.residency_peer() != Some(peer) {
                return Err(Error::Stale);
            }
            plan.edit(Some(entry), None)?;
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(
                crate::constants::MALFORMED_TX_BAN_SECONDS,
            ))
            .ok_or(Error::Fault("ban deadline"))?;
        let mut effect = Effect::rejected(hash, reject.clone(), None, false)?;
        effect.ban = Some((peer, deadline, bounded_ban_reason(&reject)));
        effect.relay = Some(TxVerificationResult::GenerationReset);
        plan.effects.push(effect);
        plan.ban = Some((peer, deadline));
        return Ok(plan);
    }
    let relay = source.residency_peer().is_some();
    plan.effects
        .push(Effect::rejected(hash, reject, None, relay)?);
    if let Some(before) = before {
        plan.edit(Some(before), None)?;
    }
    Ok(plan)
}
