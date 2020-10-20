use crate::specs::tx_pool::utils::{assert_new_block_committed, prepare_tx_family};
use crate::utils::{blank, propose};
use crate::{Node, Spec};
use ckb_types::core::BlockView;

pub struct ReorgHandleProposals;

impl Spec for ReorgHandleProposals {
    crate::setup!(num_nodes: 2);

    // Case: Check txpool handling proposals during reorg.
    //
    //
    //                    /-> A(propose tx_family.a)     // fork-A
    // genesis -> ... -> X
    //                    \-> B(propose tx_family.b)     // fork-B
    //
    //
    // Consider the above graph,
    // When a node switch the main-fork from fork-A to fork-B, `tx_family.a` becomes non-proposed,
    // and `tx_family.b` becomes proposed but unable to be committed since "parent requirement";
    // when a node switch the main-fork from fork-B to fork-A, `tx_family.b` becomes non-proposed,
    // and `tx.family.a` becomes proposed and able to be committed.
    fn run(&self, nodes: &mut Vec<Node>) {
        // 1. At the beginning, `node_a` maintains fork-A, `node_b` maintains fork-B
        let node_a = &nodes[0];
        let node_b = &nodes[1];
        let window = node_a.consensus().tx_proposal_window();

        node_a.generate_blocks(window.farthest() as usize + 2);
        let family = prepare_tx_family(node_a);
        dump_chain(node_a).iter().for_each(|block| {
            node_b.submit_block(block);
        });

        // 2. `node_a` proposes `tx_family.a`; `node_b` proposes `tx_family.b` into the
        // current proposal-window.
        // From now, fork-A and fork-B start to diverge(the common point `X` in the above graph)
        node_a.submit_transaction(&family.a());
        node_a.submit_transaction(&family.b());
        node_b.submit_transaction(&family.a());
        node_b.submit_transaction(&family.b());
        node_a.submit_block(&propose(node_a, &[family.a()]));
        node_b.submit_block(&propose(node_b, &[family.b()]));
        (0..window.closest()).for_each(|_| {
            node_a.submit_block(&blank(node_a));
        });
        (0..window.closest()).for_each(|_| {
            node_b.submit_block(&blank(node_b));
        });

        assert_new_block_committed(node_a, &[family.a().clone()]);
        assert_new_block_committed(node_b, &[]);

        // 3. `node_a` switches the main-fork from fork-A to fork-B;
        // `node_b` switches the main-fork from fork-B to fork-A;
        let a_current_blocks = dump_chain(node_a);
        let b_current_blocks = dump_chain(node_b);
        let a_next_block = blank(node_a);
        let b_next_block = blank(node_b);
        // NOTE: Append a blank `b_next_block` to `b_current_blocks`, in order to trigger reorg
        [b_current_blocks, vec![b_next_block]]
            .concat()
            .iter()
            .for_each(|block| {
                node_a.submit_block(block);
            });
        // NOTE: Append a blank `a_next_block` to `a_current_blocks`, in order to trigger reorg
        [a_current_blocks, vec![a_next_block]]
            .concat()
            .iter()
            .for_each(|block| {
                node_b.submit_block(block);
            });

        // 4. At this point, `node_a` maintains fork-B, whose valid proposals are `[]`, as
        // `tx_family.b` is invalid because of lacking its parent transaction; `node_b` maintains
        // fork-A, whose valid proposals are `[tx_family.a]` which be able to be committed.
        assert_new_block_committed(node_a, &[]);
        assert_new_block_committed(node_b, &[family.a().clone()]);
        node_a.generate_block();
        node_b.generate_block();
    }
}

fn dump_chain(node: &Node) -> Vec<BlockView> {
    (1..=node.get_tip_block_number())
        .map(|number| node.get_block_by_number(number))
        .collect()
}
