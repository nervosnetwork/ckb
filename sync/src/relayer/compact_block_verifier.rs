use crate::relayer::compact_block::CompactBlock;
use crate::{attempt, Status, StatusCode};
use ckb_core::transaction::ProposalShortId;
use std::collections::HashSet;

pub struct CompactBlockVerifier {}

impl CompactBlockVerifier {
    pub(crate) fn verify(block: &CompactBlock) -> Status {
        attempt!(PrefilledVerifier::verify(block));
        attempt!(ShortIdsVerifier::verify(block));
        StatusCode::OK.into()
    }
}

pub struct PrefilledVerifier {}

impl PrefilledVerifier {
    pub(crate) fn verify(block: &CompactBlock) -> Status {
        let prefilled_transactions = &block.prefilled_transactions;
        let short_ids = &block.short_ids;
        let txs_len = prefilled_transactions.len() + short_ids.len();

        // Check the prefilled_transactions appears to have included the cellbase
        if prefilled_transactions.is_empty() || prefilled_transactions[0].index != 0 {
            return StatusCode::MissingPrefilledCellbase.into();
        }

        // Check indices order of prefilled transactions
        let unordered = prefilled_transactions
            .as_slice()
            .windows(2)
            .any(|pt| pt[0].index >= pt[1].index);
        if unordered {
            return StatusCode::OutOfOrderPrefilledTransactions.into();
        }

        // Check highest prefilled index is less then length of block transactions
        if prefilled_transactions.last().unwrap().index >= txs_len {
            return StatusCode::OutOfIndexPrefilledTransactions.into();
        }

        Status::ok()
    }
}

pub struct ShortIdsVerifier {}

impl ShortIdsVerifier {
    pub(crate) fn verify(block: &CompactBlock) -> Status {
        let prefilled_transactions = &block.prefilled_transactions;
        let short_ids = &block.short_ids;
        let short_ids_set: HashSet<&ProposalShortId> = short_ids.iter().collect();

        // Check duplicated short ids
        if short_ids.len() != short_ids_set.len() {
            return StatusCode::DuplicatedShortIds.into();
        }

        // Check intersection of prefilled transactions and short ids
        let is_intersect = prefilled_transactions
            .iter()
            .any(|pt| short_ids_set.contains(&pt.transaction.proposal_short_id()));
        if is_intersect {
            return StatusCode::DuplicatedShortIds.into();
        }

        Status::ok()
    }
}
