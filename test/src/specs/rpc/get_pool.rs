use crate::{Node, Spec};
use ckb_types::{
    bytes::Bytes,
    packed::{CellOutput, OutPoint},
    prelude::*,
};

pub struct TxPoolEntryStatus;
pub struct TxPoolCellData;

impl Spec for TxPoolCellData {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.mine_until_out_bootstrap_period();
        let expected = Bytes::from_static(b"pool-cell-data");
        let transaction = node
            .new_transaction_spend_tip_cellbase()
            .as_advanced_builder()
            .set_outputs_data(vec![expected.pack()])
            .build();
        node.submit_transaction(&transaction);
        let point = OutPoint::new(transaction.hash(), 0);
        assert_eq!(
            node.rpc_client()
                .get_live_cell(point.clone().into(), false)
                .status,
            "unknown"
        );

        let check_data_flag = |include_tx_pool| {
            for with_data in [false, true] {
                let result = node
                    .rpc_client()
                    .inner()
                    .get_live_cell(point.clone().into(), with_data, Some(include_tx_pool))
                    .unwrap();
                assert_eq!(result.status, "live");
                let cell = result.cell.unwrap();
                assert_eq!(cell.output, transaction.output(0).unwrap().into());
                if with_data {
                    let data = cell.data.unwrap();
                    assert_eq!(data.content.as_bytes(), expected.as_ref());
                    assert_eq!(data.hash, CellOutput::calc_data_hash(&expected).into());
                } else {
                    assert!(cell.data.is_none());
                }
            }
        };
        check_data_flag(true);
        node.mine_until_transaction_confirm(&transaction.hash());
        check_data_flag(false);
    }
}

impl Spec for TxPoolEntryStatus {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        node0.new_block_with_blocking(|template| template.number.value() != 13);
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        node0.rpc_client().send_transaction(tx.data().into());
        node0.assert_pool_entry_status(tx_hash_0.clone(), "pending");
        let proposal: ckb_jsonrpc_types::ProposalShortId =
            ckb_types::packed::ProposalShortId::from_tx_hash(&tx_hash_0).into();
        // Template publication and chain reconciliation are asynchronous.
        // Establish that this block really proposes the transaction before
        // testing the resulting pool projection.
        let block =
            node0.new_block_with_blocking(|template| !template.proposals.contains(&proposal));
        node0.submit_block(&block);
        node0.get_tip_tx_pool_info();
        node0.assert_pool_entry_status(tx_hash_0.clone(), "gap");
        node0.mine(1);
        node0.get_tip_tx_pool_info();
        node0.assert_pool_entry_status(tx_hash_0, "proposed");
    }
}
