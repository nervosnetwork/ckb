use crate::util::cell::{as_input, as_inputs, as_output, as_outputs};
use crate::{Net, Node};
use ckb_network::SupportProtocols;
use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, TransactionBuilder, TransactionView},
    packed,
    prelude::*,
};

pub fn always_success_transactions(node: &Node, cells: &[CellMeta]) -> Vec<TransactionView> {
    cells
        .iter()
        .map(|cell| always_success_transaction(node, cell))
        .collect()
}

pub fn always_success_transaction(node: &Node, cell: &CellMeta) -> TransactionView {
    TransactionBuilder::default()
        .input(as_input(cell))
        .output(as_output(cell))
        .output_data(Default::default())
        .cell_dep(node.always_success_cell_dep())
        .build()
}

pub fn always_success_transactions_with_rand_data(
    node: &Node,
    cells: &[CellMeta],
) -> TransactionView {
    let len = cells.len();
    TransactionBuilder::default()
        .inputs(as_inputs(cells))
        .outputs(as_outputs(cells))
        .set_outputs_data(
            (0..len)
                .map(|_| {
                    (0..1600)
                        .map(|_| rand::random::<u8>())
                        .collect::<Vec<_>>()
                        .pack()
                })
                .collect::<Vec<packed::Bytes>>(),
        )
        .cell_dep(node.always_success_cell_dep())
        .build()
}

pub fn relay_tx(net: &Net, node: &Node, tx: TransactionView, cycles: u64) {
    let tx_hashes_msg = packed::RelayMessage::new_builder()
        .set(
            packed::RelayTransactionHashes::new_builder()
                .tx_hashes(vec![tx.hash()].pack())
                .build(),
        )
        .build();
    net.send(node, SupportProtocols::Relay, tx_hashes_msg.as_bytes());

    let ret = net.should_receive(node, |data: &Bytes| {
        packed::RelayMessage::from_slice(&data)
            .map(|message| message.to_enum().item_name() == packed::GetRelayTransactions::NAME)
            .unwrap_or(false)
    });
    assert!(ret, "node should ask for tx");

    let relay_tx = packed::RelayTransaction::new_builder()
        .cycles(cycles.pack())
        .transaction(tx.data())
        .build();

    let tx_msg = packed::RelayMessage::new_builder()
        .set(
            packed::RelayTransactions::new_builder()
                .transactions(
                    packed::RelayTransactionVec::new_builder()
                        .set(vec![relay_tx])
                        .build(),
                )
                .build(),
        )
        .build();
    net.send(node, SupportProtocols::Relay, tx_msg.as_bytes());
}
