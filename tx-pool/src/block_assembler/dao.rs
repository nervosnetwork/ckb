//! DAO field calculation for block templates.
//!
//! Split out of `block_assembler/mod.rs`; [`BlockAssembler::calc_dao`] is
//! the entry point and runs once per template transactions update.

use super::BlockAssembler;
use super::cell_liveness::{CellLivenessMemo, MemoizedChecker};
use crate::component::entry::TxEntry;
use crate::util::block_offload;
use ckb_dao::DaoCalculator;
use ckb_error::AnyError;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{Capacity, EpochExt, TransactionView, cell::TransactionsChecker},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_util::Mutex as StdMutex;
use std::collections::HashSet;
use std::iter;

/// A candidate transaction that failed the block-template resolve check,
/// with the offending out point when available.
type FailedTxs = (ProposalShortId, Option<OutPoint>);

type CalcDaoResult = Result<(Byte32, Vec<TxEntry>, Vec<FailedTxs>), AnyError>;

impl BlockAssembler {
    pub(super) fn calc_dao(
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
        cellbase: TransactionView,
        entries: Vec<TxEntry>,
        memo: &StdMutex<CellLivenessMemo>,
    ) -> CalcDaoResult {
        let tip_header = snapshot.tip_header();
        let consensus = snapshot.consensus();
        let mut seen_inputs = HashSet::new();
        let mut transactions_checker = TransactionsChecker::new(iter::once(&cellbase));

        let mut checked_failed_txs = vec![];
        let checked_entries: Vec<_> = block_offload(|| {
            entries
                .into_iter()
                .filter_map(|entry| {
                    // The chain-snapshot fallback goes through the per-tip
                    // memo; only the in-block overlay is rebuilt per entry.
                    let checker = MemoizedChecker {
                        transactions_checker: &transactions_checker,
                        snapshot,
                        memo,
                    };
                    if let Err(err) = entry.rtx.check(&mut seen_inputs, &checker, snapshot) {
                        // A permanently unresolvable proposed tx lands here
                        // on *every* template update until its ancestor
                        // lands or it expires — debug level, or it storms
                        // the error log. The caller aggregates the failed
                        // set per update.
                        debug!(
                            "Resolving transactions while building block template, \
                             tip_number: {}, tip_hash: {}, tx_hash: {}, error: {:?}",
                            tip_header.number(),
                            tip_header.hash(),
                            entry.transaction().hash(),
                            err
                        );
                        // Returning the out_point makes debugging easier and provides better logs.
                        checked_failed_txs
                            .push((entry.proposal_short_id(), err.out_point().cloned()));
                        None
                    } else {
                        transactions_checker.insert(entry.transaction());
                        Some(entry)
                    }
                })
                .collect()
        });

        let dummy_cellbase_entry = TxEntry::dummy_resolve(cellbase, 0, Capacity::zero(), 0);
        let entries_iter = iter::once(&dummy_cellbase_entry)
            .chain(checked_entries.iter())
            .map(|entry| entry.rtx.as_ref());

        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, &snapshot.borrow_as_data_loader())
            .dao_field_with_current_epoch(entries_iter, tip_header, current_epoch)?;

        Ok((dao, checked_entries, checked_failed_txs))
    }
}
