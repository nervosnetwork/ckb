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
    packed::{OutPoint, ProposalShortId},
};
use std::collections::HashMap;

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

        let mut displaced: Vec<ProposalShortId> = Vec::new();
        for input in conflict_inputs {
            if let Some((existing_fee, existing_id)) =
                self.by_input.insert(input.clone(), (fee, id.clone()))
                && existing_fee < fee
                && existing_id != id
                && !displaced.contains(&existing_id)
            {
                displaced.push(existing_id);
            }
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

    /// Remove all candidates whose conflict inputs intersect the given out-point.
    /// Useful when an in-pool tx is removed and its inputs are no longer conflicted.
    #[allow(dead_code)]
    pub fn remove_by_input(&mut self, input: &OutPoint) {
        if let Some((_, id)) = self.by_input.remove(input)
            && let Some(inputs) = self.by_id.get_mut(&id)
        {
            inputs.retain(|i| i != input);
            if inputs.is_empty() {
                self.by_id.remove(&id);
            }
        }
    }

    /// Clear all tracked candidates.
    pub fn clear(&mut self) {
        self.by_input.clear();
        self.by_id.clear();
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
}
