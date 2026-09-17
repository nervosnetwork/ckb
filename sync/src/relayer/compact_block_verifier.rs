use crate::{Status, StatusCode, attempt, relayer::MAX_RELAY_TXS_NUM_PER_BATCH};
use ckb_hash::new_blake2b;
use ckb_types::{core::ExtraHashView, packed, prelude::*};
use std::collections::HashSet;

// we assume that all the short_ids and prefilled transactions
// should NOT collide with each other,
// because in the tx-pool, the node should use short_id as the key.
pub struct CompactBlockVerifier {}

impl CompactBlockVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Status {
        attempt!(PrefilledVerifier::verify(block));
        attempt!(ShortIdsVerifier::verify(block));
        attempt!(BodyCommitmentsVerifier::verify(block));
        Status::ok()
    }
}

pub struct BodyCommitmentsVerifier {}

impl BodyCommitmentsVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Status {
        let header = block.header();
        let raw = header.raw();

        let proposals_hash = block.as_reader().proposals().calc_proposals_hash();
        if proposals_hash != raw.proposals_hash() {
            return StatusCode::CompactBlockHasInvalidHeader
                .with_context("compact proposals do not match the authenticated header");
        }

        let uncles_hash = calc_uncles_hash(&block.uncles());
        let extension_hash = block
            .extension()
            .map(|extension| extension.as_reader().calc_raw_data_hash());
        let extra_hash = ExtraHashView::new(uncles_hash, extension_hash).extra_hash();
        if extra_hash != raw.extra_hash() {
            return StatusCode::CompactBlockHasInvalidHeader
                .with_context("compact uncles or extension do not match the authenticated header");
        }

        Status::ok()
    }
}

fn calc_uncles_hash(uncles: &packed::Byte32Vec) -> packed::Byte32 {
    if uncles.is_empty() {
        packed::Byte32::zero()
    } else {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();
        for uncle in uncles.as_reader().iter() {
            blake2b.update(uncle.as_slice());
        }
        blake2b.finalize(&mut ret);
        ret.into()
    }
}

pub struct PrefilledVerifier {}

impl PrefilledVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Status {
        let prefilled_transactions = &block.prefilled_transactions();
        let short_ids = &block.short_ids();
        let txs_len = prefilled_transactions.len() + short_ids.len();

        // Check the prefilled_transactions appears to have included the cellbase
        if prefilled_transactions.is_empty() {
            return StatusCode::CompactBlockHasNotPrefilledCellbase.into();
        } else {
            // Check first prefilled index is zero
            let index: usize = prefilled_transactions.get(0).unwrap().index().into();
            if index != 0 {
                return StatusCode::CompactBlockHasNotPrefilledCellbase.into();
            }

            // Check highest prefilled index is less than length of block transactions
            let index: usize = prefilled_transactions
                .get(prefilled_transactions.len() - 1)
                .unwrap()
                .index()
                .into();
            if index >= txs_len {
                return StatusCode::CompactBlockHasOutOfIndexPrefilledTransactions.into();
            }
        }

        // Check indices order of prefilled transactions
        for i in 0..(prefilled_transactions.len() - 1) {
            let idx0: usize = prefilled_transactions.get(i).unwrap().index().into();
            let idx1: usize = prefilled_transactions.get(i + 1).unwrap().index().into();
            if idx0 >= idx1 {
                return StatusCode::CompactBlockHasOutOfOrderPrefilledTransactions.into();
            }
        }

        Status::ok()
    }
}

pub struct ShortIdsVerifier {}

impl ShortIdsVerifier {
    pub(crate) fn verify(block: &packed::CompactBlock) -> Status {
        let prefilled_transactions = block.prefilled_transactions();
        let short_ids = &block.short_ids();
        if short_ids.len() > MAX_RELAY_TXS_NUM_PER_BATCH {
            return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "ShortIds count({}) > MAX_RELAY_TXS_NUM_PER_BATCH({})",
                short_ids.len(),
                MAX_RELAY_TXS_NUM_PER_BATCH,
            ));
        }

        let short_ids_set: HashSet<packed::ProposalShortId> =
            short_ids.clone().into_iter().collect();

        // Check duplicated short ids
        if short_ids.len() != short_ids_set.len() {
            return StatusCode::CompactBlockHasDuplicatedShortIds.into();
        }

        // Check intersection of prefilled transactions and short ids.
        // Cellbase is skipped since it's always prefilled and has the chances of collision with other txs
        let is_intersect = prefilled_transactions
            .into_iter()
            .skip(1)
            .any(|pt| short_ids_set.contains(&pt.transaction().proposal_short_id()));
        if is_intersect {
            return StatusCode::CompactBlockHasDuplicatedPrefilledTransactions.into();
        }

        Status::ok()
    }
}
