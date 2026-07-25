use crate::node::waiting_for_sync_with_timeout;
use crate::specs::tx_pool::utils::{
    assert_new_block_committed, prepare_tx_family, wait_for_pending_count,
};
use crate::util::check::is_transaction_committed;
use crate::utils::{blank, propose};
use crate::{Node, Spec};
use ckb_jsonrpc_types::TxStatus;
use ckb_logger::info;
use ckb_types::core::TransactionView;
use ckb_types::packed;
use ckb_types::prelude::*;

/// Case: after a reorg removes a block that committed a parent tx, both the
/// parent and its child must be recovered into the tx-pool and be commitable
/// again.
///
/// This exercises detached replay through the synchronous direct submission
/// entry point, topologically ordered and releasing the tx-pool write lock
/// between transactions.
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
        wait_for_pending_count(node_a, 2);
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
/// This exercises the topological sort used by synchronous detached replay.
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
        assert_new_block_committed(
            node_a,
            &[grandparent.clone(), parent.clone(), child.clone()],
        );

        while node_b.get_tip_block_number() <= node_a.get_tip_block_number() {
            node_b.submit_block(&blank(node_b));
        }

        node_a.connect(node_b);
        waiting_for_sync_with_timeout(nodes, 30);
        node_a.wait_for_tx_pool();

        wait_for_pending_count(node_a, 3);
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
            &[grandparent.clone(), parent.clone(), child.clone()],
        );
    }
}

/// Case: after reorg recovers a multi-level dependent tree, the txs must be
/// **real** pending (not stranded internal Gap) and must become committable
/// again through normal `get_block_template` mining — no manual `propose()`.
///
/// This locks the regression where Gap mapped to RPC pending while
/// `get_proposals` / `TxSelector` never touched the txs, so mining only
/// ever produced cellbase + empty proposals.
pub struct ReorgRecoversDependentPendingTree;

impl Spec for ReorgRecoversDependentPendingTree {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node_a = &nodes[0];
        let node_b = &nodes[1];

        node_a.mine_until_out_bootstrap_period();

        // Fork immediately before the proposal block. After the reorg that
        // detached block has a parent on the new main chain, so it is an
        // eligible uncle carrying exactly the recovered tx proposal ids.
        node_b.connect(node_a);
        waiting_for_sync_with_timeout(nodes, 30);
        node_a.disconnect(node_b);

        let family = prepare_tx_family(node_a);
        let grandparent = family.a().clone();
        let parent = family.b().clone();
        let child = family.c().clone();
        let txs = [&grandparent, &parent, &child];

        for tx in txs {
            node_a.submit_transaction(tx);
        }

        node_a.submit_block(&propose(node_a, &txs));
        let window = node_a.consensus().tx_proposal_window();
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        assert_new_block_committed(
            node_a,
            &[grandparent.clone(), parent.clone(), child.clone()],
        );

        while node_b.get_tip_block_number() <= node_a.get_tip_block_number() {
            node_b.submit_block(&blank(node_b));
        }

        node_a.connect(node_b);
        waiting_for_sync_with_timeout(nodes, 30);
        node_a.wait_for_tx_pool();

        wait_for_pending_count(node_a, 3);
        // Distinguishes true Pending from stranded internal Gap (both look
        // like RPC `pending` via map_pool_status / pending_size).
        for tx in txs {
            node_a.assert_pool_entry_status(tx.hash(), "pending");
            assert!(
                node_a.get_transaction(tx.hash()) == TxStatus::pending(),
                "recovered tx should be RPC pending"
            );
        }

        info!("mine recovered tree through get_block_template while omitting optional uncles");
        mine_until_committed_without_template_uncles(node_a, &txs);

        for tx in txs {
            assert!(
                is_transaction_committed(node_a, tx),
                "tx {:#x} must be committed via normal mining after reorg",
                tx.hash()
            );
        }
        node_a.assert_tx_pool_size(0, 0);
    }
}

/// Mine templates while deliberately omitting optional uncles, as miners are
/// allowed to do. Recovered Pending transactions must still be proposed at the
/// top level and eventually committed; an eligible detached uncle carrying the
/// same proposal ids must not suppress them indefinitely.
fn mine_until_committed_without_template_uncles(node: &Node, txs: &[&TransactionView]) {
    let window = node.consensus().tx_proposal_window();
    // Re-propose (or uncle-propose) + closest + commit, with generous margin
    // for uncle selection order and pipeline settle time.
    let max_blocks = window
        .farthest()
        .saturating_add(window.closest())
        .saturating_add(40);

    for i in 0..max_blocks {
        if txs.iter().all(|tx| is_transaction_committed(node, tx)) {
            return;
        }

        let template = node.rpc_client().get_block_template(None, None, None);
        let block = packed::Block::from(template)
            .as_advanced_builder()
            .set_uncles(vec![])
            .build();
        node.rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .expect("submit mined template block");
        node.wait_for_tx_pool();

        if i % 10 == 9 {
            let info = node.rpc_client().tx_pool_info();
            info!(
                "mining recovered txs: block #{}, pending={}, proposed={}, orphan={}",
                block.number(),
                info.pending.value(),
                info.proposed.value(),
                info.orphan.value()
            );
        }
    }

    let info = node.rpc_client().tx_pool_info();
    let statuses: Vec<_> = txs
        .iter()
        .map(|tx| {
            let detail = node.rpc_client().get_pool_tx_detail_info(tx.hash());
            let rpc = node.get_transaction(tx.hash());
            format!(
                "{:#x}: entry_status={}, rpc={:?}",
                tx.hash(),
                detail.entry_status,
                rpc
            )
        })
        .collect();
    panic!(
        "timeout mining recovered txs via get_block_template without uncles: \
         pending={}, proposed={}, orphan={}, statuses={:?}",
        info.pending.value(),
        info.proposed.value(),
        info.orphan.value(),
        statuses
    );
}
