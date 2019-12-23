use crate::{Net, Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionView},
    prelude::*,
};
use log::info;

pub struct TemplateTxSelect;

impl Spec for TemplateTxSelect {
    crate::name!("template_tx_select");

    fn run(&self, net: &mut Net) {
        self.select_higher_tx_fee(net);
    }
}

impl TemplateTxSelect {
    fn select_higher_tx_fee(&self, net: &mut Net) {
        let node = &net.nodes[0];
        // prepare blocks
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 6) as usize);
        let mut txs_hash = Vec::new();
        let block = node.get_tip_block();
        let number = block.header().number();

        info!("Generate txs");
        // send 5 txs which tx fee rate is same
        [501, 501, 501, 501, 300]
            .iter()
            .enumerate()
            .for_each(|(i, &n)| {
                let block = node.get_block_by_number(number - i as u64);
                let cellbase = &block.transactions()[0];
                let tx = new_transaction_with_fee_and_size(
                    &node,
                    &cellbase,
                    Capacity::shannons(n as u64),
                    n as usize,
                );
                let hash = node.rpc_client().send_transaction(tx.data().into());
                txs_hash.push(hash.clone());
            });

        // skip proposal window
        node.generate_block();
        node.generate_block();

        let new_block = node.new_block(Some(1000), None, None);
        // should choose two txs: 501, 300
        assert_eq!(new_block.transactions().len(), 2);
    }
}

fn new_transaction_with_fee_and_size(
    node: &Node,
    parent_tx: &TransactionView,
    fee: Capacity,
    tx_size: usize,
) -> TransactionView {
    let input_capacity: Capacity = parent_tx
        .outputs()
        .get(0)
        .expect("parent output")
        .capacity()
        .unpack();
    let capacity = input_capacity.safe_sub(fee).unwrap();
    let tx = node.new_transaction_with_since_capacity(parent_tx.hash(), 0, capacity);
    let original_tx_size = tx.data().serialized_size_in_block();
    let tx = tx
        .as_advanced_builder()
        .set_outputs_data(vec![
            Bytes::from(vec![0u8; tx_size - original_tx_size]).pack()
        ])
        .build();
    assert_eq!(
        tx.data().serialized_size_in_block(),
        tx_size,
        "tx size incorrect"
    );
    tx
}
