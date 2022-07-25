use crate::Node;
use ckb_types::core::TransactionView;
use ckb_types::packed::{CellInput, OutPoint};
use ckb_types::prelude::*;

/// `TxFamily` used to represent a set of relative transactions,
/// `TxFamily.get(0)` is the parent of `TxFamily.get(1)`,
/// `TxFamily.get(1)` is the parent of `TxFamily.get(2)`,
/// and so on.
pub struct TxFamily {
    transactions: Vec<TransactionView>,
}

impl TxFamily {
    pub fn init(ancestor_transaction: TransactionView) -> Self {
        const FAMILY_PEOPLES: usize = 5;

        let mut transactions = vec![ancestor_transaction];
        while transactions.len() < FAMILY_PEOPLES {
            let parent = transactions.last().unwrap();
            let child = parent
                .as_advanced_builder()
                .set_inputs(vec![{
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(parent.hash(), 0))
                        .build()
                }])
                .set_outputs(vec![parent.output(0).unwrap()])
                .build();
            transactions.push(child);
        }

        TxFamily { transactions }
    }

    pub fn get(&self, index: usize) -> &TransactionView {
        self.transactions
            .get(index)
            .expect("out of index of tx-family")
    }

    #[allow(dead_code)]
    pub fn a(&self) -> &TransactionView {
        self.get(0)
    }

    #[allow(dead_code)]
    pub fn b(&self) -> &TransactionView {
        self.get(1)
    }

    #[allow(dead_code)]
    pub fn c(&self) -> &TransactionView {
        self.get(2)
    }

    #[allow(dead_code)]
    pub fn d(&self) -> &TransactionView {
        self.get(3)
    }

    #[allow(dead_code)]
    pub fn e(&self) -> &TransactionView {
        self.get(4)
    }
}

pub fn prepare_tx_family(node: &Node) -> TxFamily {
    // Ensure the generated transactions are conform to the cellbase mature rule
    let ancestor = node.new_transaction_spend_tip_cellbase();
    node.mine(node.consensus().cellbase_maturity().index());

    TxFamily::init(ancestor)
}

fn print_proposals_in_window(node: &Node) {
    let number = node.get_tip_block_number();
    let window = node.consensus().tx_proposal_window();
    let proposal_start = number.saturating_sub(window.farthest()) + 1;
    let proposal_end = number.saturating_sub(window.closest()) + 1;
    for number in proposal_start..=proposal_end {
        let block = node.get_block_by_number(number);
        println!(
            "\tBlock[#{}].proposals: {:?}",
            number,
            block.union_proposal_ids()
        );
    }
}

pub fn assert_new_block_committed(node: &Node, committed: &[TransactionView]) {
    let block = node.new_block(None, None, None);
    if committed != &block.transactions()[1..] {
        print_proposals_in_window(node);
        assert_eq!(committed, &block.transactions()[1..]);
    }
}
