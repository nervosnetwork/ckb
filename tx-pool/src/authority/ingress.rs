//! Source promotion and current-owner rejection plans.
use super::{
    jobs::Resolution,
    model::{Entry, Error, FullReason, Phase, Source},
    notice::{Class, Effect},
    store::{Plan, Store},
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
    let mut plan = Plan::new(view, class(source), Default::default());
    let current = plan.get(store, &transaction.hash())?;
    if let Source::Remote { peer, .. } = source
        && plan.peer_banned(store, peer)?
    {
        plan.notify(Effect::relay(TxVerificationResult::Reject {
            tx_hash: compact_packed(&transaction.hash()),
        }));
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
        return rejection(store, plan, None, &transaction.hash(), source, reject);
    }
    match (&current, source) {
        (Some(old), Source::Remote { peer, .. }) => {
            plan.notify(Effect::relay(if old.accepted().is_some() {
                TxVerificationResult::Ok {
                    original_peer: Some(peer),
                    tx_hash: compact_packed(&transaction.hash()),
                }
            } else {
                TxVerificationResult::Reject {
                    tx_hash: compact_packed(&transaction.hash()),
                }
            }));
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
    plan.edit(current, Some(after), None)?;
    Ok(plan)
}

/// A resolution result determines its owner phase, original reads and required
/// parent request together. The worker only commits or retries this outcome.
pub(super) fn resolution(
    store: &Store,
    view: u64,
    before: &Arc<Entry>,
    result: &Resolution,
) -> Result<Plan, Error> {
    let (phase, reads) = match result {
        Resolution::Ready(resolved) => (Phase::Verify(Arc::clone(resolved)), &resolved.reads),
        Resolution::Waiting(keys, reads) => (Phase::Waiting(keys.clone()), reads),
        Resolution::Rejected(reject, reads) => {
            return rejection(
                store,
                Plan::new(view, class(before.source), reads.clone()),
                Some(Arc::clone(before)),
                &before.hash(),
                before.source,
                reject.clone(),
            );
        }
    };
    let mut plan = Plan::new(view, class(before.source), reads.clone());
    let after = before.with_phase(phase);
    let effect = Effect::waiting(&after);
    plan.edit(Some(Arc::clone(before)), Some(after), effect)?;
    Ok(plan)
}

/// Capacity refusal observes the current owner without removing it, and still
/// owes a rejection notice that releases the relayer's pending filter.
pub(super) fn capacity_rejection(
    store: &Store,
    hash: &Byte32,
    source: Source,
    reason: FullReason,
) -> Result<Plan, Error> {
    let (view, _) = store.snapshot();
    let mut plan = Plan::new(view, class(source), Default::default());
    plan.get(store, hash)?;
    rejection(
        store,
        plan,
        None,
        hash,
        source,
        Reject::Full(reason.to_string()),
    )
}

/// A remote malformed result revokes exactly the current preaccepted cohort.
/// The original culprit read prevents an old worker from banning after promotion.
pub(super) fn rejection(
    store: &Store,
    plan: Plan,
    before: Option<Arc<Entry>>,
    hash: &Byte32,
    source: Source,
    reject: Reject,
) -> Result<Plan, Error> {
    let mut plan = plan.discard_changes();
    if let Some(before) = &before {
        plan.observe_owner(hash, Some(before))?;
    }
    if reject.is_malformed_tx()
        && let Source::Remote {
            peer,
            cycles: Some(_),
            ..
        } = source
    {
        let hashes = plan.peer_members(store, peer)?;
        for hash in hashes {
            let entry = plan.get(store, &hash)?.ok_or(Error::Stale)?;
            if !entry.preaccepted() || entry.source.residency_peer() != Some(peer) {
                return Err(Error::Stale);
            }
            plan.edit(Some(entry), None, None)?;
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(
                crate::constants::MALFORMED_TX_BAN_SECONDS,
            ))
            .ok_or(Error::Fault("ban deadline"))?;
        plan.ban_peer(hash, reject, peer, deadline)?;
        return Ok(plan);
    }
    let effect =
        Effect::candidate_rejected(hash, reject, source, before.as_deref(), &store.budget)?;
    if let Some(before) = before {
        plan.edit(Some(before), None, Some(effect))?;
    } else {
        plan.notify(effect);
    }
    Ok(plan)
}
