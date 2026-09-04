use super::*;
use crate::{authority::state::RawTxHash, constants::MAX_READY_BATCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum CandidateBatchError {
    Empty,
    TooLarge { limit: usize },
    Duplicate(RawTxHash),
}

impl SettlementBatch {
    pub(in crate::authority) fn new(
        candidates: Vec<FinalAdmissionReceipt>,
    ) -> Result<Self, CandidateBatchError> {
        if candidates.is_empty() {
            return Err(CandidateBatchError::Empty);
        }
        if candidates.len() > MAX_READY_BATCH {
            return Err(CandidateBatchError::TooLarge {
                limit: MAX_READY_BATCH,
            });
        }
        for (index, candidate) in candidates.iter().enumerate() {
            if candidates
                .iter()
                .skip(index.saturating_add(1))
                .any(|other| other.key() == candidate.key())
            {
                return Err(CandidateBatchError::Duplicate(candidate.key().clone()));
            }
        }
        let mut candidates = candidates.into_iter();
        let Some(head) = candidates.next() else {
            return Err(CandidateBatchError::Empty);
        };
        Ok(Self {
            head,
            tail: candidates.collect(),
        })
    }
}
