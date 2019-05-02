use crate::relayer::compact_block::CompactBlock;
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

        // Check indices order of prefilled transactions
        let mut prev = prefilled_transactions
            .get(0)
            .and_then(|it| Some(it.index))
            .unwrap_or(0);
        for index_transaction in prefilled_transactions.iter().skip(1) {
            if prev >= index_transaction.index {
                return Err(Error::UnorderedPrefilledTransactions);
            }
            prev = index_transaction.index;
        }

        // Check highest prefilled index is less then length of block transactions
        if prev >= txs_len {
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
        let short_ids_set: HashSet<[u8; 6]> = short_ids.iter().map(Clone::clone).collect();
        let txs_len = prefilled_transactions.len() + short_ids.len();

        // Check empty transactions
        if txs_len == 0 {
            return Err(Error::EmptyTransactions);
        }

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
