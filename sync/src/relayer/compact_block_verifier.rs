use crate::relayer::error::{Error, Misbehavior};
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;

// we assume that all the short_ids and prefilled transactions
// should NOT collide with each other,
// because in the tx-pool, the node should use short_id as the key.
pub struct CompactBlockVerifier {}

impl CompactBlockVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Result<(), Error> {
        PrefilledVerifier::verify(block)?;
        ShortIdsVerifier::verify(block)?;
        Ok(())
    }
}

pub struct PrefilledVerifier {}

impl PrefilledVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Result<(), Error> {
        let prefilled_transactions = &block.prefilled_transactions();
        let short_ids = &block.short_ids();
        let txs_len = prefilled_transactions.len() + short_ids.len();

        // Check the prefilled_transactions appears to have included the cellbase
        if prefilled_transactions.is_empty() {
            return Err(Error::Misbehavior(Misbehavior::CellbaseNotPrefilled));
        }
        let index: usize = prefilled_transactions.get(0).unwrap().index().unpack();
        if index != 0 {
            return Err(Error::Misbehavior(Misbehavior::CellbaseNotPrefilled));
        }

        // Check indices order of prefilled transactions
        for i in 0..(prefilled_transactions.len() - 1) {
            let idx0: usize = prefilled_transactions.get(i).unwrap().index().unpack();
            let idx1: usize = prefilled_transactions.get(i + 1).unwrap().index().unpack();
            if idx0 >= idx1 {
                return Err(Error::Misbehavior(
                    Misbehavior::UnorderedPrefilledTransactions,
                ));
            }
        }

        // Check highest prefilled index is less then length of block transactions
        if !prefilled_transactions.is_empty() {
            let index: usize = prefilled_transactions
                .get(prefilled_transactions.len() - 1)
                .unwrap()
                .index()
                .unpack();
            if index >= txs_len {
                return Err(Error::Misbehavior(
                    Misbehavior::OverflowPrefilledTransactions,
                ));
            }
        }

        Ok(())
    }
}

pub struct ShortIdsVerifier {}

impl ShortIdsVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Result<(), Error> {
        let prefilled_transactions = block.prefilled_transactions();
        let short_ids = &block.short_ids();
        let short_ids_set: HashSet<packed::ProposalShortId> =
            short_ids.clone().into_iter().collect();

        // Check duplicated short ids
        if short_ids.len() != short_ids_set.len() {
            return Err(Error::Misbehavior(Misbehavior::DuplicatedShortIds));
        }

        // Check intersection of prefilled transactions and short ids
        let is_intersect = prefilled_transactions
            .into_iter()
            .any(|pt| short_ids_set.contains(&pt.transaction().proposal_short_id()));
        if is_intersect {
            return Err(Error::Misbehavior(
                Misbehavior::IntersectedPrefilledTransactions,
            ));
        }

        Ok(())
    }
}
