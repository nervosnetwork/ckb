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

pub fn prepare_txfamily(node: &Node) -> TxFamily {
    // Ensure the generated transactions are conform to the cellbase mature rule
    let ancestor = node.new_transaction_spend_tip_cellbase();
    node.generate_blocks(node.consensus().cellbase_maturity().index() as usize);

    TxFamily::init(ancestor)
}
