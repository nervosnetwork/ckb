use crate::utils::{build_relay_tx_hashes, wait_until};
use crate::{Net, Node, Spec};
use ckb_constant::sync::MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
use ckb_network::SupportProtocols;
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;

pub struct TooManyUnknownTransactions;

impl Spec for TooManyUnknownTransactions {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        node0.mine(4);
        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV3],
        );
        net.connect(node0);

        let limit = MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
        let tx_hashes: Vec<_> = (0..=limit)
            .map(|index| {
                let mut bytes = [0; 32];
                bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
                packed::Byte32::from(bytes)
            })
            .collect();

        // Fill all but one slot, then repeat those hashes alongside the final new hash.
        // Observed requests establish that both announcements have been processed.
        for (announced, expected) in [
            (&tx_hashes[..limit - 1], &tx_hashes[..limit - 1]),
            (&tx_hashes[..limit], &tx_hashes[limit - 1..limit]),
        ] {
            net.send(
                node0,
                SupportProtocols::RelayV3,
                build_relay_tx_hashes(announced),
            );
            let mut missing: HashSet<_> = expected.iter().cloned().collect();
            let requested = net.should_receive(node0, |data| {
                if let Ok(packed::RelayMessageUnion::GetRelayTransactions(request)) =
                    packed::RelayMessage::from_slice(data).map(|message| message.to_enum())
                {
                    for hash in request.tx_hashes() {
                        missing.remove(&hash);
                    }
                }
                missing.is_empty()
            });
            assert!(requested, "node did not request {} hashes", missing.len());
            assert!(
                node0.rpc_client().get_banned_addresses().is_empty(),
                "announcements within the unique-hash limit must not ban the peer"
            );
        }

        // Exceed the retained-hash limit with a valid one-hash protocol message.
        net.send(
            node0,
            SupportProtocols::RelayV3,
            build_relay_tx_hashes(&tx_hashes[limit..]),
        );

        let banned = wait_until(60, || node0.rpc_client().get_banned_addresses().len() == 1);
        assert!(
            banned,
            "NetController should be banned cause TooManyUnknownTransactions"
        );
        assert!(
            node0.rpc_client().get_banned_addresses()[0]
                .ban_reason
                .contains("TooManyUnknownTransactions"),
            "the ban must be caused by request capacity, not message size"
        );
    }
}
