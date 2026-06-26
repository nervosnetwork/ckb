use crate::node::waiting_for_sync_with_timeout;
use crate::specs::tx_pool::utils::{assert_new_block_committed, prepare_tx_family};
use crate::utils::{blank, propose};
use crate::{Node, Spec};
use ckb_jsonrpc_types::TxStatus;

/// Case: after a reorg removes a block that committed a parent tx, both the
/// parent and its child must be recovered into the tx-pool and be commitable
/// again.
///
/// This exercises the V2 pipeline reorg path where detached transactions are
/// recovered in dependency order through the synchronous `_process_tx` entry
/// point, releasing the tx-pool write lock between transactions.
pub struct ReorgRecoversDependentTxs;

impl Spec for ReorgRecoversDependentTxs {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node_a = &nodes[0];
        let node_b = &nodes[1];

        // 1. Bootstrap chain on node_a.
        node_a.mine_until_out_bootstrap_period();

        // 2. Build a dependent tx chain on node_a and submit both txs.
        let family = prepare_tx_family(node_a);
        let parent = family.a();
        let child = family.b();

        node_a.submit_transaction(parent);
        node_a.submit_transaction(child);

        // 3. node_a proposes and commits the parent, while node_b mines a
        //    competing longer chain that does NOT include the parent.
        node_a.submit_block(&propose(node_a, &[parent]));
        let window = node_a.consensus().tx_proposal_window();
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        assert_new_block_committed(node_a, std::slice::from_ref(parent));

        // node_b keeps mining blank blocks until it is strictly longer than node_a.
        while node_b.get_tip_block_number() <= node_a.get_tip_block_number() {
            node_b.submit_block(&blank(node_b));
        }

        // 4. Connect; node_a must reorg to node_b's longer chain.
        node_a.connect(node_b);
        waiting_for_sync_with_timeout(nodes, 30);
        node_a.wait_for_tx_pool();

        // 5. Both parent and child should be back in the pool.
        let info = node_a.rpc_client().tx_pool_info();
        assert_eq!(
            info.pending.value(),
            2,
            "parent and child should be recovered to pending after reorg"
        );
        assert!(node_a.get_transaction(parent.hash()) == TxStatus::pending());
        assert!(node_a.get_transaction(child.hash()) == TxStatus::pending());

        // 6. The recovered chain must be commitable again.
        node_a.submit_block(&propose(node_a, &[parent, child]));
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        assert_new_block_committed(node_a, &[parent.clone(), child.clone()]);
    }
}

/// Case: after a reorg removes a block that committed a multi-level dependent
/// chain, all txs (grandparent, parent, child) must be recovered in dependency
/// order and be commitable again.
///
/// This exercises the topological sort in the V2 pipeline reorg path.
pub struct ReorgRecoversDependentChain;

impl Spec for ReorgRecoversDependentChain {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node_a = &nodes[0];
        let node_b = &nodes[1];

        node_a.mine_until_out_bootstrap_period();

        let family = prepare_tx_family(node_a);
        let grandparent = family.a();
        let parent = family.b();
        let child = family.c();

        node_a.submit_transaction(grandparent);
        node_a.submit_transaction(parent);
        node_a.submit_transaction(child);

        node_a.submit_block(&propose(node_a, &[grandparent, parent, child]));
        let window = node_a.consensus().tx_proposal_window();
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        assert_new_block_committed(node_a, &[grandparent.clone(), parent.clone(), child.clone()]);

        while node_b.get_tip_block_number() <= node_a.get_tip_block_number() {
            node_b.submit_block(&blank(node_b));
        }

        node_a.connect(node_b);
        waiting_for_sync_with_timeout(nodes, 30);
        node_a.wait_for_tx_pool();

        let info = node_a.rpc_client().tx_pool_info();
        assert_eq!(
            info.pending.value(),
            3,
            "grandparent, parent and child should be recovered to pending after reorg"
        );
        assert!(node_a.get_transaction(grandparent.hash()) == TxStatus::pending());
        assert!(node_a.get_transaction(parent.hash()) == TxStatus::pending());
        assert!(node_a.get_transaction(child.hash()) == TxStatus::pending());

        node_a.submit_block(&propose(node_a, &[grandparent, parent, child]));
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        assert_new_block_committed(
            node_a,
            &[
                grandparent.clone(),
                parent.clone(),
                child.clone(),
            ],
        );
    }
}
