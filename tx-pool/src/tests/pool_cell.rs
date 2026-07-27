use super::TxPoolResolvedCellChecker;
use ckb_types::{
    core::{EpochNumberWithFraction, TransactionInfo, cell::CellChecker},
    packed::{Byte32, OutPoint},
};
use std::cell::Cell;

struct ProbeChecker {
    answer: Option<bool>,
    calls: Cell<usize>,
}

impl ProbeChecker {
    fn new(answer: Option<bool>) -> Self {
        Self {
            answer,
            calls: Cell::new(0),
        }
    }
}

impl CellChecker for ProbeChecker {
    fn is_live(&self, _out_point: &OutPoint) -> Option<bool> {
        self.calls.set(self.calls.get() + 1);
        self.answer
    }
}

fn resolved_chain_cell(out_point: OutPoint) -> ckb_types::core::cell::CellMeta {
    ckb_types::core::cell::CellMetaBuilder::default()
        .out_point(out_point)
        .transaction_info(TransactionInfo::new(
            1,
            EpochNumberWithFraction::new(0, 0, 1),
            Byte32::zero(),
            0,
        ))
        .build()
}

#[test]
fn tx_pool_same_tip_resolution_evidence_skips_chain_revalidation() {
    let tip = Byte32::new([1; 32]);
    let overlay = ProbeChecker::new(None);
    let chain = ProbeChecker::new(Some(false));
    let checker = TxPoolResolvedCellChecker::new(&overlay, &chain, &tip, &tip);
    let cell = resolved_chain_cell(OutPoint::new(Byte32::new([2; 32]), 0));

    assert_eq!(checker.is_live_resolved_cell(&cell), Some(true));
    assert_eq!(overlay.calls.get(), 1);
    assert_eq!(chain.calls.get(), 0);
}

#[test]
fn tx_pool_resolution_evidence_yields_to_pool_spends_and_tip_changes() {
    let resolved_tip = Byte32::new([1; 32]);
    let current_tip = Byte32::new([2; 32]);
    let cell = resolved_chain_cell(OutPoint::new(Byte32::new([3; 32]), 0));

    let spent_overlay = ProbeChecker::new(Some(false));
    let unused_chain = ProbeChecker::new(Some(true));
    let same_tip =
        TxPoolResolvedCellChecker::new(&spent_overlay, &unused_chain, &resolved_tip, &resolved_tip);
    assert_eq!(same_tip.is_live_resolved_cell(&cell), Some(false));
    assert_eq!(unused_chain.calls.get(), 0);

    let empty_overlay = ProbeChecker::new(None);
    let changed_chain = ProbeChecker::new(Some(false));
    let changed_tip =
        TxPoolResolvedCellChecker::new(&empty_overlay, &changed_chain, &resolved_tip, &current_tip);
    assert_eq!(changed_tip.is_live_resolved_cell(&cell), Some(false));
    assert_eq!(changed_chain.calls.get(), 1);
}

#[test]
fn tx_pool_resolution_evidence_requires_chain_provenance() {
    let tip = Byte32::new([1; 32]);
    let overlay = ProbeChecker::new(None);
    let chain = ProbeChecker::new(Some(false));
    let checker = TxPoolResolvedCellChecker::new(&overlay, &chain, &tip, &tip);
    let cell = ckb_types::core::cell::CellMetaBuilder::default()
        .out_point(OutPoint::new(Byte32::new([4; 32]), 0))
        .build();

    assert_eq!(checker.is_live_resolved_cell(&cell), Some(false));
    assert_eq!(chain.calls.get(), 1);
}
