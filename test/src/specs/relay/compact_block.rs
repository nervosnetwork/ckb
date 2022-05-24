use crate::node::waiting_for_sync;
use crate::util::cell::gen_spendable;
use crate::util::check::is_transaction_committed;
use crate::util::mining::out_ibd_mode;
use crate::util::transaction::always_success_transaction;
use crate::utils::{
    build_block, build_block_transactions, build_compact_block, build_compact_block_with_prefilled,
    build_header, build_headers, wait_until,
};
use crate::{Net, Node, Spec};
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_types::{
    core::{HeaderBuilder, HeaderView},
    h256,
    packed::{
        self, GetHeaders, RelayMessage, RelayMessageUnion::GetBlockTransactions, SyncMessage,
        SyncMessageUnion::GetBlocks,
    },
    prelude::*,
};

pub struct CompactBlockEmptyParentUnknown;

impl Spec for CompactBlockEmptyParentUnknown {
    // Case: Sent to node0 a parent-unknown empty block, node0 should be unable to reconstruct
    // it and send us back a `GetHeaders` message
    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        let node = &nodes[0];
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node);

        node.mine(1);

        let parent_unknown_block = node
            .new_block_builder(None, None, None)
            .header(
                HeaderBuilder::default()
                    .parent_hash(h256!("0x123456").pack())
                    .build(),
            )
            .build();
        let tip_block = node.get_tip_block();
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block(&parent_unknown_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() != tip_block);
        assert!(!ret, "Node0 should reconstruct empty block failed");

        let ret = net.should_receive(node, |data: &Bytes| {
            SyncMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Node0 should send back GetHeaders message for unknown parent header"
        );
    }
}

pub struct CompactBlockEmpty;

impl Spec for CompactBlockEmpty {
    // Case: Send to node0 a parent-known empty block, node0 should be able to reconstruct it
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        out_ibd_mode(nodes);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node);

        let new_empty_block = node.new_block(None, None, None);
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block(&new_empty_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_empty_block);
        assert!(ret, "Node0 should reconstruct empty block successfully");
    }
}

pub struct CompactBlockPrefilled;

impl Spec for CompactBlockPrefilled {
    // Case: Send to node0 a block with all transactions prefilled, node0 should be able to reconstruct it
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        out_ibd_mode(nodes);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node);

        let cells = gen_spendable(node, 1);
        let new_tx = always_success_transaction(node, &cells[0]);
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.mine(3);

        // Relay a block contains `new_tx` as committed
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block_with_prefilled(&new_block, vec![1]),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(
            ret,
            "Node0 should reconstruct all-prefilled block successfully"
        );
    }
}

pub struct CompactBlockMissingFreshTxs;

impl Spec for CompactBlockMissingFreshTxs {
    // Case: Send to node0 a block which missing a tx, which is a fresh tx for
    // tx_pool, node0 should send `GetBlockTransactions` back for requesting
    // these missing txs
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        out_ibd_mode(nodes);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node);

        let cells = gen_spendable(node, 1);
        let new_tx = always_success_transaction(node, &cells[0]);
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.mine(3);

        // Relay a block contains `new_tx` as committed, but not include in prefilled
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(!ret, "Node0 should be unable to reconstruct the block");

        let ret = net.should_receive(node, |data: &Bytes| {
            let get_block_txns = RelayMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == packed::GetBlockTransactions::NAME)
                .unwrap_or(false);
            let get_block = SyncMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == packed::GetBlocks::NAME)
                .unwrap_or(false);
            get_block_txns || get_block
        });
        assert!(
            ret,
            "Node0 should send GetBlockTransactions message for missing transactions"
        );
    }
}

pub struct CompactBlockMissingNotFreshTxs;

impl Spec for CompactBlockMissingNotFreshTxs {
    // Case: As for the missing transactions of a compact block, we should try to find it from
    //       tx_pool. If we find out, we can reconstruct the target block without any requests
    //       to the peer.
    // 1. Put the target tx into tx_pool, and proposal it. Then move it into proposal window
    // 2. Relay target block which contains the target transaction as committed transaction. Expect
    //    successful to reconstruct the target block and grow up.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        out_ibd_mode(nodes);
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node);

        // Build the target transaction
        let cells = gen_spendable(node, 1);
        let new_tx = always_success_transaction(node, &cells[0]);
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.mine(3);

        // Generate the target block which contains the target transaction as a committed transaction
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx.clone())
            .build();

        // Put `new_tx` as an not fresh tx into tx_pool
        node.rpc_client().send_transaction(new_tx.data().into());

        // Relay the target block
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(ret, "Node0 should be able to reconstruct the block");
    }
}

/// Test case:
/// 1. CompactBlock new with 2 tx commit, but local node has only one, send GetBlockTransactions to get the missed one
/// 2. At this time, node lost its tx on tx-pool
/// 3. Received BlockTransactions with requets one, but can't construct the block, try requests with all 2 tx
/// 4. Received BlockTransactions with two txs, constract block success
pub struct CompactBlockMissingWithDropTx;

impl Spec for CompactBlockMissingWithDropTx {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.mine(3);

        // Build the target transaction
        let cells = gen_spendable(node, 2);
        let new_tx_1 = always_success_transaction(node, &cells[0]);
        let new_tx_2 = always_success_transaction(node, &cells[1]);

        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposals(vec![
                    new_tx_1.proposal_short_id(),
                    new_tx_2.proposal_short_id(),
                ])
                .build(),
        );

        node.mine(3);

        // Generate the target block which contains the target transaction as a committed transaction
        let new_block = node
            .new_block_builder(None, None, None)
            .transactions(vec![new_tx_1.clone(), new_tx_2.clone()])
            .build();

        // Put `new_tx` as an not fresh tx into tx_pool
        node.rpc_client().send_transaction(new_tx_1.data().into());

        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::RelayV2],
        );
        net.connect(node);

        // Relay the target block
        net.send(
            node,
            SupportProtocols::RelayV2,
            build_compact_block(&new_block),
        );

        let ret = net.should_receive(node, |data| {
            RelayMessage::from_slice(data)
                .map(|message| {
                    if let GetBlockTransactions(get_block_transactions) = message.to_enum() {
                        let msg = get_block_transactions
                            .as_reader()
                            .indexes()
                            .iter()
                            .map(|i| Unpack::<u32>::unpack(&i))
                            .collect::<Vec<u32>>();
                        vec![2] == msg
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Node should send GetBlockTransactions message for missing transactions"
        );

        // Remove tx1 on tx pool
        node.rpc_client().remove_transaction(new_tx_1.hash());

        let content = packed::BlockTransactions::new_builder()
            .block_hash(new_block.hash())
            .transactions(vec![new_tx_2.data()].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();

        // Send tx2 to node
        net.send(node, SupportProtocols::RelayV2, message.as_bytes());

        let ret = net.should_receive(node, |data| {
            RelayMessage::from_slice(data)
                .map(|message| {
                    if let GetBlockTransactions(get_block_transactions) = message.to_enum() {
                        let msg = get_block_transactions
                            .as_reader()
                            .indexes()
                            .iter()
                            .map(|i| Unpack::<u32>::unpack(&i))
                            .collect::<Vec<u32>>();
                        vec![1, 2] == msg
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        });

        assert!(
            ret,
            "Node should send GetBlockTransactions message with 2 tx missing transactions"
        );

        let content = packed::BlockTransactions::new_builder()
            .block_hash(new_block.hash())
            .transactions(vec![new_tx_1.data(), new_tx_2.data()].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();

        // send tx1 and tx2 to node
        net.send(node, SupportProtocols::RelayV2, message.as_bytes());

        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(ret, "Node should be able to reconstruct the block");
    }
}

pub struct CompactBlockLoseGetBlockTransactions;

impl Spec for CompactBlockLoseGetBlockTransactions {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        let node0 = &nodes[0];
        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node0);
        let node1 = &nodes[1];
        net.connect(node1);

        let cells = gen_spendable(node0, 1);
        let new_tx = always_success_transaction(node0, &cells[0]);
        node0.submit_block(
            &node0
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        // Proposal a tx, and grow up into proposal window
        node0.mine(6);

        // Make node0 and node1 reach the same height
        node1.mine(1);
        node0.connect(node1);
        waiting_for_sync(&[node0, node1]);

        // Construct a new block contains one transaction
        let block = node0
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();

        // Net send the compact block to node0, but dose not send the corresponding missing
        // block transactions. It will make node0 unable to reconstruct the complete block
        net.send(
            node0,
            SupportProtocols::RelayV2,
            build_compact_block(&block),
        );

        let ret = net.should_receive(node0, |data: &Bytes| {
            let get_block_txns = RelayMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == packed::GetBlockTransactions::NAME)
                .unwrap_or(false);
            let get_block = SyncMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == packed::GetBlocks::NAME)
                .unwrap_or(false);
            get_block_txns || get_block
        });
        assert!(
            ret,
            "Node0 should send GetBlockTransactions message for missing transactions"
        );

        // Submit the new block to node1. We expect node1 will relay the new block to node0.
        node1.submit_block(&block);
        waiting_for_sync(&[node0, node1]);
    }
}

pub struct BlockTransactionsRelayParentOfOrphanBlock;

impl Spec for BlockTransactionsRelayParentOfOrphanBlock {
    crate::setup!(num_nodes: 2);

    // Case: A == B.parent, A.transactions is not empty
    // 1. Sync SentBlock-B to node0. Node0 will put B into orphan_block_pool since B's parent is unknown
    // 2. Relay CompactBlock-A to node0. Node0 misses fresh transactions of A, hence it will request for
    // the missing A.transactions via GetBlockTransactions
    // 3. Relay BlockTransactions-A to node0. Node0 will process A, and by the way process B that was
    // inserted in orphan_block_pool before
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let block_a = {
            let cells = gen_spendable(node1, 1);
            let tx = always_success_transaction(node1, &cells[0]);
            node1.submit_transaction(&tx);
            node1.mine_until_bool(|| is_transaction_committed(node1, &tx));
            node1.get_tip_block()
        };
        node1.mine(1);
        let block_b = node1.get_tip_block();

        for number in 1..block_a.number() {
            let block = node1.get_block_by_number(number);
            node0.submit_block(&block);
        }

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node0);

        // 1. Sync block_b's SendBlock to node0. It has the following steps:
        //   (1) Sync block_a's SendHeader to node0
        //   (2) Sync block_b's SendHeader to node0
        //   (3) Wait GetBlocks from node0
        //   (4) Sync block_b's SendBlock to node0
        // After then node0 should save block_b into orphan_block_pool as it only receives block_a's
        // header but lack of block_a's body.
        net.send(
            node0,
            SupportProtocols::Sync,
            build_header(&block_a.header()),
        );
        net.send(
            node0,
            SupportProtocols::Sync,
            build_header(&block_b.header()),
        );
        let ret = net.should_receive(node0, |data| {
            SyncMessage::from_slice(data)
                .map(|message| {
                    if let GetBlocks(get_blocks) = message.to_enum() {
                        for hash in get_blocks.block_hashes().into_iter() {
                            if hash == block_b.hash() {
                                return true;
                            }
                        }
                    }
                    false
                })
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Node0 should receive GetBlocks which consists of `block_b`"
        );
        net.send(node0, SupportProtocols::Sync, build_block(&block_b));

        // 2. Relay CompactBlock-A to node0. Node0 misses fresh transactions of A, hence it will request for
        // the missing A.transactions via GetBlockTransactions
        net.send(
            node0,
            SupportProtocols::RelayV2,
            build_compact_block(&block_a),
        );
        let ret = net.should_receive(node0, |data| {
            RelayMessage::from_slice(data)
                .map(|message| {
                    if let GetBlockTransactions(get_block_transactions) = message.to_enum() {
                        get_block_transactions.block_hash() == block_a.hash()
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Node0 should request block_a's missing transactions via GetBlockTransactions"
        );

        // 3. Relay BlockTransactions-A to node0. Node0 will process A, and by the way process B that was
        // inserted in orphan_block_pool before
        net.send(
            node0,
            SupportProtocols::RelayV2,
            build_block_transactions(&block_a),
        );

        let ret = wait_until(20, || node0.get_tip_block_number() == block_b.number());
        if !ret {
            assert_eq!(
                node0.get_tip_block_number(),
                block_b.number(),
                "relayer should process the two blocks, including the orphan block"
            );
        }
    }
}

pub struct CompactBlockRelayParentOfOrphanBlock;

impl Spec for CompactBlockRelayParentOfOrphanBlock {
    crate::setup!(num_nodes: 2);

    // Case: A == B.parent
    // 1. Sync B to node0. Node0 will put B into orphan_block_pool since B's parent is unknown
    // 2. Relay A to node0. Node0 will process A, and by the way process B that was inserted in
    // orphan_block_pool before
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        out_ibd_mode(nodes);

        let (block_a, block_b) = {
            let node1 = &nodes[1];
            node1.mine(2);
            let tip_number = node1.get_tip_block_number();
            (
                node1.get_block_by_number(tip_number - 1),
                node1.get_block_by_number(tip_number),
            )
        };

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node0);

        // 1. Sync block_b's SendBlock to node0. It has the following steps:
        //   (1) Sync block_a's SendHeader to node0
        //   (2) Sync block_b's SendHeader to node0
        //   (3) Wait GetBlocks from node0
        //   (4) Sync block_b's SendBlock to node0
        // After then node0 should save block_b into orphan_block_pool as it only receives block_a's
        // header but lack of block_a's body.
        net.send(
            node0,
            SupportProtocols::Sync,
            build_header(&block_a.header()),
        );
        net.send(
            node0,
            SupportProtocols::Sync,
            build_header(&block_b.header()),
        );
        let ret = net.should_receive(node0, |data| {
            SyncMessage::from_slice(data)
                .map(|message| {
                    if let GetBlocks(get_blocks) = message.to_enum() {
                        for hash in get_blocks.block_hashes().into_iter() {
                            if hash == block_b.hash() {
                                return true;
                            }
                        }
                    }
                    false
                })
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Node0 should receive GetBlocks which consists of `block_b`"
        );
        net.send(node0, SupportProtocols::Sync, build_block(&block_b));

        // 2. Relay block_a's CompactBlock to node0
        net.send(
            node0,
            SupportProtocols::RelayV2,
            build_compact_block(&block_a),
        );

        let ret = wait_until(20, || node0.get_tip_block_number() == block_b.number());
        if !ret {
            assert_eq!(
                node0.get_tip_block_number(),
                block_b.number(),
                "relayer should process the two blocks, including the orphan block"
            );
        }
    }
}

pub struct CompactBlockRelayLessThenSharedBestKnown;

impl Spec for CompactBlockRelayLessThenSharedBestKnown {
    crate::setup!(num_nodes: 2);

    // Case: Relay a compact block which has lower total difficulty than shared_best_known
    // 1. Synchronize Headers[Tip+1, Tip+10]
    // 2. Relay CompactBlock[Tip+1]
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        out_ibd_mode(nodes);
        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::RelayV2],
        );
        net.connect(node0);

        assert_eq!(node0.get_tip_block(), node1.get_tip_block());
        let old_tip = node1.get_tip_block_number();
        node1.mine(10);
        let headers: Vec<HeaderView> = (old_tip + 1..node1.get_tip_block_number())
            .map(|i| node1.rpc_client().get_header_by_number(i).unwrap().into())
            .collect();
        net.send(node0, SupportProtocols::Sync, build_headers(&headers));

        let ret = net.should_receive(node0, |data| {
            SyncMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == packed::GetBlocks::NAME)
                .unwrap_or(false)
        });
        assert!(ret, "Node0 should send GetBlocks message");

        let new_block = node0.new_block(None, None, None);
        net.send(
            node0,
            SupportProtocols::RelayV2,
            build_compact_block(&new_block),
        );
        assert!(
            wait_until(20, move || node0.get_tip_block().header().number() == old_tip + 1),
            "node0 should process the new block, even its difficulty is less than best_shared_known",
        );
    }
}
