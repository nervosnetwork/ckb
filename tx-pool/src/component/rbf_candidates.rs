//! Tracks in-flight RBF replacement candidates so that conflicting replacements
//! are ordered by fee rate before they reach `submit_entry`.
//!
//! When multiple remote transactions try to replace the same in-pool
//! transaction(s), the one with the highest fee rate should win.  Without an
//! ordering gate, a lower-fee-rate candidate can finish pre-check/verification
//! first, enter the pool, and then block a later higher-fee-rate candidate
//! because the incremental RBF fee rule is calculated against the
//! lower-fee-rate candidate rather than the original transaction.
//!
//! This module keeps a lightweight index of the highest-fee-rate candidate per
//! conflict input.  A candidate is registered after pre-check succeeds and
//! before it enters the verify queue.  If a higher-fee-rate candidate for the
//! same input is already registered, the current candidate is rejected
//! immediately.  Before `submit_entry` finalizes a replacement, it checks the
//! index again; if a higher-fee-rate candidate has appeared in the meantime, the
//! current candidate aborts.  The candidate is unregistered after `submit_entry`
//! finishes, whether it succeeded or failed.
//!
//! # Fee-rate unit
//!
//! The fee rates stored here are *size-based* (`FeeRate::calculate(fee,
//! tx_size)`), the same unit used by `min_fee_rate` and by the RBF replacement
//! fee floor ([`TxPool::calculate_min_replace_fee`]).
//!
//! A weight-based rate would incorporate peer-declared cycles, but those are
//! not verified until the verify stage (`DeclaredWrongCycles`): a malicious
//! peer could declare artificially low cycles to inflate its weight-based fee
//! rate and displace honest candidates before the lie is caught. Using the
//! size-based rate removes that manipulable input from the ordering gate.

use ckb_logger::debug;
use ckb_types::{
    core::FeeRate,
    packed::{OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet};

/// Lightweight size-based fee-rate-ordering gate for in-flight RBF
/// replacements.
///
/// See the module-level documentation for why the gate uses the same
/// size-based fee-rate unit as the pool's fee checks.
#[derive(Default)]
pub(crate) struct RbfCandidates {
    /// Highest size-based fee-rate candidate currently known for each conflict
    /// input.
    by_input: HashMap<OutPoint, (FeeRate, ProposalShortId)>,
    /// Reverse index so we can clean up by candidate id when it leaves the
    /// pipeline or is removed by management commands.
    by_id: HashMap<ProposalShortId, Vec<OutPoint>>,
}

/// A pending RBF registration that has been validated but not yet committed.
///
/// The caller must either call [`RbfCandidates::commit`] after the candidate
/// has been successfully added to the verify queue, or drop the registration
/// (which leaves `RbfCandidates` unchanged). This makes displacement atomic
/// with verify-queue insertion: lower-fee-rate candidates are only removed
/// from the pipeline once the higher-fee-rate candidate is guaranteed to enter
/// it.
#[derive(Debug)]
pub(crate) struct RbfRegistration {
    new_id: ProposalShortId,
    new_fee_rate: FeeRate,
    new_conflict_inputs: Vec<OutPoint>,
    /// Full state of displaced candidates so they can be removed from the
    /// verify queue and their RBF registrations atomically committed.
    pub(crate) displaced: Vec<(ProposalShortId, FeeRate, Vec<OutPoint>)>,
}

impl RbfCandidates {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            by_input: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    /// Validate a candidate and compute the registration delta without mutating
    /// the index.
    ///
    /// `&mut self` enforces that this is called while holding `rbf_candidates.write()`.
    /// The caller should commit the registration after the candidate has been
    /// successfully inserted into the verify queue.  This makes displacement
    /// atomic with verify-queue insertion and avoids losing displaced candidates
    /// if the insertion fails. Returns `Err` if a higher-or-equal-fee-rate
    /// candidate is already registered for any input.
    ///
    /// `fee_rate` must be the size-based fee rate (see module-level docs);
    /// comparisons inside the index assume all stored rates use the same unit.
    pub fn register(
        &mut self,
        id: ProposalShortId,
        fee_rate: FeeRate,
        conflict_inputs: &[OutPoint],
    ) -> Result<RbfRegistration, String> {
        for input in conflict_inputs {
            if let Some((existing_fee_rate, existing_id)) = self.by_input.get(input)
                && (*existing_fee_rate > fee_rate
                    || (*existing_fee_rate == fee_rate && *existing_id != id))
            {
                debug!(
                    "RBF candidate {} fee_rate {} rejected: input {:?} already held by {} fee_rate {}",
                    id, fee_rate, input, existing_id, existing_fee_rate
                );
                return Err(format!(
                    "input {:?} already has higher-or-equal-fee-rate RBF candidate {}",
                    input, existing_id
                ));
            }
        }

        // Collect unique lower-fee-rate candidates that are displaced by this
        // new candidate, including their full state so they can be removed from
        // the verify queue once the registration is committed.
        let mut displaced: Vec<(ProposalShortId, FeeRate, Vec<OutPoint>)> = Vec::new();
        let mut seen: HashSet<ProposalShortId> = HashSet::new();
        for input in conflict_inputs {
            if let Some((existing_fee_rate, existing_id)) = self.by_input.get(input)
                && *existing_fee_rate < fee_rate
                && existing_id != &id
                && seen.insert(existing_id.clone())
                && let Some(inputs) = self.by_id.get(existing_id)
            {
                displaced.push((existing_id.clone(), *existing_fee_rate, inputs.clone()));
            }
        }

        Ok(RbfRegistration {
            new_id: id,
            new_fee_rate: fee_rate,
            new_conflict_inputs: conflict_inputs.to_vec(),
            displaced,
        })
    }

    /// Atomically commit a pending registration: remove displaced candidates
    /// from the index and install the new candidate.
    ///
    /// Must be called while holding `rbf_candidates.write()`.
    pub fn commit(&mut self, registration: RbfRegistration) {
        let RbfRegistration {
            new_id,
            new_fee_rate,
            new_conflict_inputs,
            displaced,
        } = registration;

        // Fully unregister each displaced candidate from *all* of its indexed
        // inputs.  A displaced candidate may cover more inputs than the new
        // candidate overlaps with; leaving those entries behind would cause
        // later replacements for the other inputs to be rejected by a candidate
        // that is no longer in flight.
        for (displaced_id, _, _) in &displaced {
            if let Some(inputs) = self.by_id.remove(displaced_id) {
                for input in inputs {
                    if self
                        .by_input
                        .get(&input)
                        .is_some_and(|(_, candidate_id)| candidate_id == displaced_id)
                    {
                        self.by_input.remove(&input);
                    }
                }
            }
        }

        for input in &new_conflict_inputs {
            self.by_input
                .insert(input.clone(), (new_fee_rate, new_id.clone()));
        }
        self.by_id.insert(new_id, new_conflict_inputs);
    }

    /// Returns true if a higher size-based fee-rate candidate has been
    /// registered for any of the given conflict inputs.  Equal-fee-rate entries
    /// are allowed only when the id matches (i.e. the candidate is checking
    /// itself).
    pub fn is_superseded(
        &self,
        id: &ProposalShortId,
        fee_rate: FeeRate,
        conflict_inputs: &[OutPoint],
    ) -> bool {
        conflict_inputs.iter().any(|input| {
            self.by_input
                .get(input)
                .is_some_and(|(existing_fee_rate, existing_id)| {
                    *existing_fee_rate > fee_rate
                        || (*existing_fee_rate == fee_rate && existing_id != id)
                })
        })
    }

    /// Remove a candidate by id.  Idempotent.
    pub fn remove(&mut self, id: &ProposalShortId) {
        if let Some(inputs) = self.by_id.remove(id) {
            for input in inputs {
                if self
                    .by_input
                    .get(&input)
                    .is_some_and(|(_, candidate_id)| candidate_id == id)
                {
                    self.by_input.remove(&input);
                }
            }
        }
    }

    /// Clear all tracked candidates.
    pub fn clear(&mut self) {
        self.by_input.clear();
        self.by_id.clear();
    }

    /// Remove candidates whose conflict inputs reference any of the given
    /// outpoints. Called after those outpoints have been freed by committed,
    /// evicted, or replaced transactions so the stale candidates do not block
    /// future replacements.
    pub fn remove_by_conflict_outpoints(&mut self, outpoints: &HashSet<OutPoint>) {
        let ids_to_remove: Vec<ProposalShortId> = self
            .by_id
            .iter()
            .filter(|(_id, inputs)| inputs.iter().any(|input| outpoints.contains(input)))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids_to_remove {
            self.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{h256, prelude::Pack};

    fn out_point(idx: u8) -> OutPoint {
        OutPoint::new(
            h256!("0x0101010101010101010101010101010101010101010101010101010101010101").pack(),
            idx as u32,
        )
    }

    fn id(idx: u8) -> ProposalShortId {
        let bytes = [idx; 32];
        ProposalShortId::from_tx_hash(&ckb_types::packed::Byte32::new(bytes))
    }

    #[test]
    fn highest_fee_rate_wins() {
        let mut rbf = RbfCandidates::new();
        let input = out_point(0);
        let id_a = id(0);
        let id_b = id(1);

        let reg = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                std::slice::from_ref(&input),
            )
            .unwrap();
        rbf.commit(reg);
        // Lower-fee-rate candidate is rejected.
        assert!(
            rbf.register(
                id_b.clone(),
                FeeRate::from_u64(50),
                std::slice::from_ref(&input)
            )
            .is_err()
        );
        assert!(rbf.is_superseded(&id_b, FeeRate::from_u64(50), std::slice::from_ref(&input)));
        // Higher-fee-rate candidate supersedes.
        assert!(!rbf.is_superseded(&id_b, FeeRate::from_u64(200), std::slice::from_ref(&input)));

        // Remove the old candidate and register a new one.
        rbf.remove(&id_a);
        let reg = rbf
            .register(id_b, FeeRate::from_u64(50), std::slice::from_ref(&input))
            .unwrap();
        rbf.commit(reg);
    }

    #[test]
    fn multiple_inputs_all_checked() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let input1 = out_point(1);
        let id_a = id(0);
        let id_b = id(1);

        let reg = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                &[input0.clone(), input1.clone()],
            )
            .unwrap();
        rbf.commit(reg);

        // Candidate that only conflicts with one input but with lower fee rate
        // is rejected.
        assert!(
            rbf.register(
                id_b.clone(),
                FeeRate::from_u64(50),
                std::slice::from_ref(&input0)
            )
            .is_err()
        );

        // Higher-fee-rate candidate can take over both inputs.
        let reg = rbf
            .register(id_b.clone(), FeeRate::from_u64(200), &[input0, input1])
            .unwrap();
        let displaced_ids: Vec<_> = reg.displaced.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(displaced_ids, vec![id_a]);
        rbf.commit(reg);
        // Removing the new candidate frees both.
        rbf.remove(&id_b);
        assert!(rbf.by_input.is_empty());
    }

    #[test]
    fn displaced_candidate_is_fully_unregistered() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let input1 = out_point(1);
        let id_a = id(0);
        let id_b = id(1);
        let id_c = id(2);

        // Candidate A covers two inputs.
        let reg = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                &[input0.clone(), input1.clone()],
            )
            .unwrap();
        rbf.commit(reg);

        // Candidate B only overlaps input0 but has a higher fee rate.  It must
        // displace A entirely, including input1 which B does not touch.
        let reg = rbf
            .register(
                id_b.clone(),
                FeeRate::from_u64(200),
                std::slice::from_ref(&input0),
            )
            .unwrap();
        let displaced_ids: Vec<_> = reg.displaced.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(displaced_ids, vec![id_a.clone()]);
        rbf.commit(reg);

        // input1 must no longer be held by the displaced candidate A, otherwise
        // a later replacement for input1 would be rejected by a ghost candidate.
        assert_eq!(rbf.by_input.get(&input1), None);
        assert_eq!(rbf.by_id.get(&id_a), None);

        // A new candidate for input1 (previously held only by A) should succeed.
        let reg = rbf
            .register(
                id_c.clone(),
                FeeRate::from_u64(150),
                std::slice::from_ref(&input1),
            )
            .unwrap();
        rbf.commit(reg);

        // Cleanup.
        rbf.remove(&id_b);
        rbf.remove(&id_c);
        assert!(rbf.by_input.is_empty());
        assert!(rbf.by_id.is_empty());
    }

    #[test]
    fn remove_by_conflict_outpoints_cleans_stale_candidates() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let input1 = out_point(1);
        let input2 = out_point(2);
        let id_a = id(0);
        let id_b = id(1);

        // Candidate A conflicts with input0.
        let reg_a = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                std::slice::from_ref(&input0),
            )
            .unwrap();
        rbf.commit(reg_a);
        // Candidate B conflicts with input1 and input2.
        let reg_b = rbf
            .register(
                id_b.clone(),
                FeeRate::from_u64(100),
                &[input1.clone(), input2.clone()],
            )
            .unwrap();
        rbf.commit(reg_b);

        // A committed transaction spends input0 and input2. Both A and B should
        // be removed because their conflict inputs are now consumed on-chain.
        let committed_outpoints: HashSet<OutPoint> =
            [input0.clone(), input2.clone()].into_iter().collect();
        rbf.remove_by_conflict_outpoints(&committed_outpoints);

        assert_eq!(rbf.by_id.get(&id_a), None);
        assert_eq!(rbf.by_id.get(&id_b), None);
        assert_eq!(rbf.by_input.get(&input0), None);
        assert_eq!(rbf.by_input.get(&input2), None);

        // input1 is untouched, so its reverse index entry should also be gone
        // because candidate B was fully removed.
        assert_eq!(rbf.by_input.get(&input1), None);
    }

    #[test]
    fn remove_by_conflict_outpoints_keeps_untouched_candidates() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let input1 = out_point(1);
        let id_a = id(0);
        let id_b = id(1);

        let reg_a = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                std::slice::from_ref(&input0),
            )
            .unwrap();
        rbf.commit(reg_a);
        let reg_b = rbf
            .register(
                id_b.clone(),
                FeeRate::from_u64(200),
                std::slice::from_ref(&input1),
            )
            .unwrap();
        rbf.commit(reg_b);

        // Only input0 is spent; candidate B must remain untouched.
        rbf.remove_by_conflict_outpoints(&[input0.clone()].into_iter().collect());
        assert_eq!(rbf.by_id.get(&id_a), None);
        assert!(rbf.by_id.contains_key(&id_b));
        assert!(rbf.by_input.contains_key(&input1));
        assert_eq!(rbf.by_input.get(&input0), None);
    }

    #[test]
    fn remove_by_conflict_outpoints_empty_is_noop() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let id_a = id(0);

        let reg = rbf
            .register(
                id_a.clone(),
                FeeRate::from_u64(100),
                std::slice::from_ref(&input0),
            )
            .unwrap();
        rbf.commit(reg);

        rbf.remove_by_conflict_outpoints(&HashSet::new());
        assert!(rbf.by_id.contains_key(&id_a));
        assert!(rbf.by_input.contains_key(&input0));
    }
}
