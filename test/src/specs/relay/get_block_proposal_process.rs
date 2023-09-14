use crate::util::cell::{gen_spendable, get_spendable};
use crate::util::mining::out_ibd_mode;
use crate::util::transaction::always_success_transactions_with_rand_data;
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_types::{packed, prelude::*};

pub struct ProposalRespondSizelimit;

impl Spec for ProposalRespondSizelimit {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        let node0 = &nodes[0];

        let mut cells = gen_spendable(node0, 100);
        let data_size: u64 = 1600 * 100;

        let mut proposal_ids = Vec::new();

        let mut transaction = always_success_transactions_with_rand_data(node0, &cells);

        for _ in 0..4 * 1024 * 1024 / data_size + 1 {
            node0.submit_transaction(&transaction);

            proposal_ids.push(transaction.proposal_short_id());

            node0.mine_until_transaction_confirm(&transaction.hash());

            // spend all new cell
            cells = get_spendable(node0);
            transaction = always_success_transactions_with_rand_data(node0, &cells);
        }

        assert!(proposal_ids.len() < 3000);

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV3],
        );

        let tip = node0.get_tip_block_number();
        let tip_hash = node0.rpc_client().get_block_hash(tip).unwrap();

        let content = packed::GetBlockProposal::new_builder()
            .block_hash(tip_hash)
            .proposals(proposal_ids.pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();

        net.connect(node0);

        net.send(node0, SupportProtocols::RelayV3, message.as_bytes());

        assert!(
            node0.rpc_client().get_banned_addresses().is_empty(),
            "net should not banned"
        );
        let res = net.should_receive(node0, |data| {
            packed::RelayMessage::from_slice(data)
                .map(|message| {
                    if let packed::RelayMessageUnion::BlockProposal(inner) = message.to_enum() {
                        inner.as_slice().len() < 1024 * 1024
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        });

        assert!(
            node0.rpc_client().get_banned_addresses().is_empty(),
            "net should not banned"
        );
        assert!(res, "block proposal responde size must less than 1M")
    }
}
