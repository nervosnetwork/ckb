//! Tracks in-flight RBF replacement candidates so that conflicting replacements
//! are ordered by fee before they reach `submit_entry`.
//!
//! When multiple remote transactions try to replace the same in-pool
//! transaction(s), the one with the highest fee should win.  Without an
//! ordering gate, a lower-fee candidate can finish pre-check/verification first,
//! enter the pool, and then block a later higher-fee candidate because the
//! incremental RBF fee rule is calculated against the lower-fee candidate rather
//! than the original transaction.
//!
//! This module keeps a lightweight index of the highest-fee candidate per
//! conflict input.  A candidate is registered after pre-check succeeds and
//! before it enters the verify queue.  If a higher-fee candidate for the same
//! input is already registered, the current candidate is rejected immediately.
//! Before `submit_entry` finalizes a replacement, it checks the index again; if
//! a higher-fee candidate has appeared in the meantime, the current candidate
//! aborts.  The candidate is unregistered after `submit_entry` finishes, whether
//! it succeeded or failed.

use ckb_logger::debug;
use ckb_types::{
    core::Capacity,
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet};

/// Lightweight fee-ordering gate for in-flight RBF replacements.
#[derive(Default)]
pub(crate) struct RbfCandidates {
    /// Highest-fee candidate currently known for each conflict input.
    by_input: HashMap<OutPoint, (Capacity, ProposalShortId)>,
    /// Reverse index so we can clean up by candidate id when it leaves the
    /// pipeline or is removed by management commands.
    by_id: HashMap<ProposalShortId, Vec<OutPoint>>,
}

impl RbfCandidates {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            by_input: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    /// Attempt to register a candidate.  Returns `Ok(displaced_ids)` (possibly
    /// empty) if registration succeeded; the vector contains **all** lower-fee
    /// candidates that were displaced across every conflict input.  Returns
    /// `Err` if a higher-fee candidate is already registered for any input.
    pub fn register(
        &mut self,
        id: ProposalShortId,
        fee: Capacity,
        conflict_inputs: &[OutPoint],
    ) -> Result<Vec<ProposalShortId>, String> {
        for input in conflict_inputs {
            if let Some((existing_fee, existing_id)) = self.by_input.get(input)
                && (*existing_fee > fee || (*existing_fee == fee && *existing_id != id))
            {
                debug!(
                    "RBF candidate {} fee {} rejected: input {:?} already held by {} fee {}",
                    id, fee, input, existing_id, existing_fee
                );
                return Err(format!(
                    "input {:?} already has higher-fee RBF candidate {}",
                    input, existing_id
                ));
            }
        }

        // Collect unique lower-fee candidates that are displaced by this new
        // candidate.
        let mut displaced: Vec<ProposalShortId> = Vec::new();
        for input in conflict_inputs {
            if let Some((existing_fee, existing_id)) = self.by_input.get(input)
                && *existing_fee < fee
                && existing_id != &id
                && !displaced.contains(existing_id)
            {
                displaced.push(existing_id.clone());
            }
        }

        // Fully unregister each displaced candidate from *all* of its indexed
        // inputs.  A displaced candidate may cover more inputs than the new
        // candidate overlaps with; leaving those entries behind would cause
        // later replacements for the other inputs to be rejected by a candidate
        // that is no longer in flight.
        for displaced_id in &displaced {
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

        for input in conflict_inputs {
            self.by_input.insert(input.clone(), (fee, id.clone()));
        }
        self.by_id.insert(id, conflict_inputs.to_vec());
        Ok(displaced)
    }

    /// Returns true if a higher-fee candidate has been registered for any of the
    /// given conflict inputs.  Equal-fee entries are allowed only when the id
    /// matches (i.e. the candidate is checking itself).
    pub fn is_superseded(
        &self,
        id: &ProposalShortId,
        fee: Capacity,
        conflict_inputs: &[OutPoint],
    ) -> bool {
        conflict_inputs.iter().any(|input| {
            self.by_input
                .get(input)
                .is_some_and(|(existing_fee, existing_id)| {
                    *existing_fee > fee || (*existing_fee == fee && existing_id != id)
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
    /// transaction hashes. Called after those transactions are committed so
    /// the stale candidates do not block future replacements.
    pub fn remove_by_conflict_tx_hashes(&mut self, tx_hashes: &HashSet<Byte32>) {
        let ids_to_remove: Vec<ProposalShortId> = self
            .by_id
            .iter()
            .filter(|(_id, inputs)| {
                inputs
                    .iter()
                    .any(|input| tx_hashes.contains(&input.tx_hash()))
            })
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
    fn highest_fee_wins() {
        let mut rbf = RbfCandidates::new();
        let input = out_point(0);
        let id_a = id(0);
        let id_b = id(1);

        rbf.register(
            id_a.clone(),
            Capacity::shannons(100),
            std::slice::from_ref(&input),
        )
        .unwrap();
        // Lower-fee candidate is rejected.
        assert!(
            rbf.register(
                id_b.clone(),
                Capacity::shannons(50),
                std::slice::from_ref(&input)
            )
            .is_err()
        );
        assert!(rbf.is_superseded(&id_b, Capacity::shannons(50), std::slice::from_ref(&input)));
        // Higher-fee candidate supersedes.
        assert!(!rbf.is_superseded(&id_b, Capacity::shannons(200), std::slice::from_ref(&input)));

        // Remove the old candidate and register a new one.
        rbf.remove(&id_a);
        assert!(
            rbf.register(id_b, Capacity::shannons(50), std::slice::from_ref(&input))
                .is_ok()
        );
    }

    #[test]
    fn multiple_inputs_all_checked() {
        let mut rbf = RbfCandidates::new();
        let input0 = out_point(0);
        let input1 = out_point(1);
        let id_a = id(0);
        let id_b = id(1);

        rbf.register(
            id_a.clone(),
            Capacity::shannons(100),
            &[input0.clone(), input1.clone()],
        )
        .unwrap();

        // Candidate that only conflicts with one input but with lower fee is rejected.
        assert!(
            rbf.register(
                id_b.clone(),
                Capacity::shannons(50),
                std::slice::from_ref(&input0)
            )
            .is_err()
        );

        // Higher-fee candidate can take over both inputs.
        let displaced = rbf
            .register(id_b.clone(), Capacity::shannons(200), &[input0, input1])
            .unwrap();
        assert_eq!(displaced, vec![id_a]);
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
        rbf.register(
            id_a.clone(),
            Capacity::shannons(100),
            &[input0.clone(), input1.clone()],
        )
        .unwrap();

        // Candidate B only overlaps input0 but has a higher fee.  It must
        // displace A entirely, including input1 which B does not touch.
        let displaced = rbf
            .register(
                id_b.clone(),
                Capacity::shannons(200),
                std::slice::from_ref(&input0),
            )
            .unwrap();
        assert_eq!(displaced, vec![id_a.clone()]);

        // input1 must no longer be held by the displaced candidate A, otherwise
        // a later replacement for input1 would be rejected by a ghost candidate.
        assert_eq!(rbf.by_input.get(&input1), None);
        assert_eq!(rbf.by_id.get(&id_a), None);

        // A new candidate for input1 (previously held only by A) should succeed.
        rbf.register(
            id_c.clone(),
            Capacity::shannons(150),
            std::slice::from_ref(&input1),
        )
        .unwrap();

        // Cleanup.
        rbf.remove(&id_b);
        rbf.remove(&id_c);
        assert!(rbf.by_input.is_empty());
        assert!(rbf.by_id.is_empty());
    }
}
