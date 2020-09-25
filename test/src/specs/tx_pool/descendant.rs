use crate::specs::tx_pool::utils::prepare_tx_family;
use crate::utils::{blank, commit, propose};
use crate::{Node, Spec};
use std::collections::HashSet;

// Convention:
//   * `tx_family` is a set of relative transactions, `tx_family.a <- tx_family.b <-
//     tx_family.c <- tx_family.d <- tx_family.e`, note that `x <- y` represents `x` is the parent
//     transaction of `y`.

pub struct HandlingDescendantsOfProposed;

impl Spec for HandlingDescendantsOfProposed {
    // Case: This case intends to test the handling of proposed transactions.
    //       We construct a scenario that although both `tx_family.a` and `tx_family.b` are in
    //       txpool, but only propose `tx_family.a`. We expect that after proposing
    //       `tx_family.a`, miner is able to propose `tx_family.b` in the next blocks.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Put `tx_family` into pending-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_transaction(family.b());

        // 2. Propose `tx_family.a` only, then we expect `tx_family.b` being proposed in the next
        //    blocks, even after `tx_family.a` expiring, out of `tx_family.a`'s proposal window
        node.submit_block(&propose(node, &[family.a()]));
        (0..=window.farthest() + window.closest()).for_each(|_| {
            let block = node.new_block(None, None, None);
            assert!(
                block
                    .union_proposal_ids()
                    .contains(&family.b().proposal_short_id()),
                "Miner should propose tx_family.b since it has never been proposed, actual: {:?}",
                block.union_proposal_ids(),
            );

            node.submit_block(&blank(node)); // continuously submit blank blocks.
        });

        // 3. At this point, `tx_family.a` has been moved in pending-pool since it is
        //    out of proposal-window. Hence miner will propose `tx_family.a` and `tx_family.b`
        //    in the next blocks.
        let block = node.new_block(None, None, None);
        assert_eq!(
            vec![
                family.a().proposal_short_id(),
                family.b().proposal_short_id()
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
            block.union_proposal_ids(),
        );
    }
}

pub struct HandlingDescendantsOfCommitted;

impl Spec for HandlingDescendantsOfCommitted {
    // Case: This case intends to test the handling descendants of committed transactions.
    //       We construct a scenario that although both `tx_family.a` and `tx_family.b` are in
    //       txpool, but only propose and commit `tx_family.a`. We expect that after proposing
    //       `tx_family.a`, miner is able to propose `tx_family.b` in the next blocks.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Put `tx_family` into pending-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_transaction(family.b());

        // 2. Propose and commit `tx_family.a` only
        node.submit_block(&propose(node, &[family.a()]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        }); // continuously submit blank blocks.
        node.submit_block(&commit(node, &[family.a()]));

        let block = node.new_block(None, None, None);
        assert_eq!(
            vec![family.b().proposal_short_id()]
                .into_iter()
                .collect::<HashSet<_>>(),
            block.union_proposal_ids(),
        );
        node.submit_block(&block);
    }
}

pub struct ProposeOutOfOrder;

impl Spec for ProposeOutOfOrder {
    // Case: Even if the proposals is out of order of relatives(child transaction
    //       proposed before its parent transaction), miner commits in order of
    //       relatives
    //   1. Put `tx_family` into pending-pool.
    //   2. Propose `[tx_family.b, tx_family.a]`, then continuously submit blank blocks.
    //   3. Expect committing `[tx_family.a, tx_family.b]`.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Put `tx_family` into pending-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_transaction(family.b());

        // 2. Propose `[tx_family.b, tx_family.a]`, then continuously submit blank blocks.
        node.submit_block(&propose(node, &[family.b(), family.a()]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node)); // continuously submit blank blocks.
        });

        // 3. Expect committing `[tx_family.a, tx_family.b]`.
        let block = node.new_block(None, None, None);
        assert_eq!(
            [family.a().to_owned(), family.b().to_owned()],
            block.transactions()[1..],
        );
        node.submit_block(&block);
    }
}

pub struct SubmitTransactionWhenItsParentInGap;

impl Spec for SubmitTransactionWhenItsParentInGap {
    // Case: This case intends to test that submit a transaction which its parent is in gap-pool
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Propose `tx_family.a` into gap-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_block(&propose(node, &[family.a()]));

        // 2. Submit `tx_family.b` into pending-pool. Then we expect that miner propose it.
        node.submit_transaction(family.b());
        let block = node.new_block(None, None, None);
        assert!(
            block
                .union_proposal_ids()
                .contains(&family.b().proposal_short_id()),
            "Miner should propose tx_family.b since it has never been proposed, actual: {:?}",
            block.union_proposal_ids(),
        );
        node.submit_block(&block);
    }
}

pub struct SubmitTransactionWhenItsParentInProposed;

impl Spec for SubmitTransactionWhenItsParentInProposed {
    // Case: This case intends to test that submit a transaction which its parent is in proposed-pool
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Propose `tx_family.a` into proposed-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_block(&propose(node, &[family.a()]));
        (0..=window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });

        // 2. Submit `tx_family.b` into pending-pool. Then we expect that miner propose it.
        node.submit_transaction(family.b());
        let block = node.new_block(None, None, None);
        assert!(
            block
                .union_proposal_ids()
                .contains(&family.b().proposal_short_id()),
            "Miner should propose tx_family.b since it has never been proposed, actual: {:?}",
            block.union_proposal_ids(),
        );
        node.submit_block(&block);
    }
}

pub struct ProposeTransactionButParentNot;

impl Spec for ProposeTransactionButParentNot {
    // Case: A proposed transaction cannot be committed if its parent has not been committed
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks((window.farthest() + 2) as usize);

        // 1. Propose `tx_family.a` and `tx_family.b` into pending-pool.
        let family = prepare_tx_family(node);
        node.submit_transaction(family.a());
        node.submit_transaction(family.b());

        // 2. Propose `tx_family.b`, but `tx_family.a` not, then continuously submit blank blocks.
        //    In the time, miner should not commit `tx_family.b` as its parent, `tx_family.a` has
        //    not been not proposed and committed yet.
        node.submit_block(&propose(node, &[family.b()]));
        (0..window.closest()).for_each(|_| {
            node.submit_block(&blank(node)); // continuously submit blank blocks.
        });
        let block = node.new_block(None, None, None);
        assert_eq!(block.transactions()[1..], []);

        let block = commit(node, &[family.b()]);
        node.rpc_client()
            .submit_block("".to_string(), block.data().into())
            .expect_err("should be failed as it contains invalid transaction");
    }
}
