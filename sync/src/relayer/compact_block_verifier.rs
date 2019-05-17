use crate::relayer::compact_block::{CompactBlock, ShortTransactionID};
use crate::relayer::error::Error;
use ckb_protocol::{short_transaction_id, short_transaction_id_keys};
use std::collections::HashSet;

pub struct CompactBlockVerifier {
    prefilled: PrefilledVerifier,
    short_ids: ShortIdsVerifier,
}

impl CompactBlockVerifier {
    pub(crate) fn new() -> Self {
        Self {
            prefilled: PrefilledVerifier::new(),
            short_ids: ShortIdsVerifier::new(),
        }
    }

    pub(crate) fn verify(&self, block: &CompactBlock) -> Result<(), Error> {
        self.prefilled.verify(block)?;
        self.short_ids.verify(block)?;
        Ok(())
    }
}

pub struct PrefilledVerifier {}

impl PrefilledVerifier {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) fn verify(&self, block: &CompactBlock) -> Result<(), Error> {
        let prefilled_transactions = &block.prefilled_transactions;
        let short_ids = &block.short_ids;
        let txs_len = prefilled_transactions.len() + short_ids.len();

        // Check the prefilled_transactions appears to have included the cellbase
        if prefilled_transactions.is_empty() || prefilled_transactions[0].index != 0 {
            return Err(Error::CellbaseNotPrefilled);
        }

        // Check indices order of prefilled transactions
        let unordered = prefilled_transactions
            .as_slice()
            .windows(2)
            .any(|pt| pt[0].index >= pt[1].index);
        if unordered {
            return Err(Error::UnorderedPrefilledTransactions);
        }

        // Check highest prefilled index is less then length of block transactions
        if !prefilled_transactions.is_empty()
            && prefilled_transactions.last().unwrap().index >= txs_len
        {
            return Err(Error::OverflowPrefilledTransactions);
        }

        Ok(())
    }
}

pub struct ShortIdsVerifier {}

impl ShortIdsVerifier {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) fn verify(&self, block: &CompactBlock) -> Result<(), Error> {
        let prefilled_transactions = &block.prefilled_transactions;
        let short_ids = &block.short_ids;
        let short_ids_set: HashSet<&ShortTransactionID> = short_ids.iter().collect();

        // Check duplicated short ids
        if short_ids.len() != short_ids_set.len() {
            return Err(Error::DuplicatedShortIds);
        }

        // Check intersection of prefilled transactions and short ids
        let (key0, key1) = short_transaction_id_keys(block.header.nonce(), block.nonce);
        let is_intersect = prefilled_transactions.iter().any(|it| {
            let short_id = short_transaction_id(key0, key1, &it.transaction.witness_hash());
            short_ids_set.contains(&short_id)
        });
        if is_intersect {
            return Err(Error::IntersectedPrefilledTransactions);
        }

        Ok(())
    }
}
