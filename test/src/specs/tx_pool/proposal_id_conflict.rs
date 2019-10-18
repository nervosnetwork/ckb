use crate::specs::tx_pool::utils::TxFamily;
use crate::utils::{blank, commit, propose};
use crate::{Net, Node, Spec};
use ckb_jsonrpc_types::TxStatus;
use ckb_types::bytes::Bytes;
use ckb_types::core::{BlockNumber, BlockView, Capacity, TransactionView};
use ckb_types::packed::{self, Byte32, ProposalShortId};
use ckb_types::prelude::*;
use ckb_types::{h256, H256};

const FAMILY_SIZE: usize = 3;

pub struct FakeHashTransaction;

impl Spec for FakeHashTransaction {
    crate::name!("fake_hash_transaction");

    // Case: This case is just intended to make sure that RPC `send_mock_transaction` and
    // `send_mock_transaction` works fine, which means that the way of mocking transaction
    // do not break the validation rules. It helps eliminate the side-effect of
    // fake-hash-transaction. If this case has been pass, then we expect that the rest below
    // cases be passed too.
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 3);

        // x: x_tx_family
        // y: y_tx_family
        // x.a() is irrelevant with y.a(),
        // but x.a().proposal_id() == y.a().proposal_id()
        let (x_tx_family, y_tx_family) = irrelevant_collided_tx_families(node);

        send_tx_family(node, &x_tx_family);
        propose_tx_family(node, &x_tx_family);
        (0..=window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });
        commit_tx_family(node, &x_tx_family);

        send_tx_family(node, &y_tx_family);
        propose_tx_family(node, &y_tx_family);
        (0..=window.closest()).for_each(|_| {
            node.submit_block(&blank(node));
        });
        commit_tx_family(node, &y_tx_family);

        assert_committed(node, &x_tx_family);
        assert_committed(node, &y_tx_family);
    }
}

pub struct ProposalIDCollisionOnPendingIrrelevant;

impl Spec for ProposalIDCollisionOnPendingIrrelevant {
    crate::name!("proposal_id_collision_on_pending_irrelevant");

    crate::setup!(num_nodes: 2, connect_all: false);

    fn run(&self, net: &mut Net) {
        let generate_node = &net.nodes[0];
        let test_node = &net.nodes[1];
        let window = generate_node.consensus().tx_proposal_window();
        (0..=window.farthest() + 2).for_each(|_| {
            let block = generate_node.new_block(None, None, None);
            generate_node.submit_block(&block);
            test_node.submit_block(&block);
        });

        // x: x_tx_family
        // y: y_tx_family
        // x.a() is irrelevant with y.a(),
        // but x.a().proposal_id() == y.a().proposal_id()
        let (x_tx_family, y_tx_family) = irrelevant_collided_tx_families(generate_node);

        // Prepare: Generate delta chain: [propose(x)] -> [] -> [] -> [commit(x)].
        //
        // After that, family y should be pending state in the pending-pool.
        send_tx_family(generate_node, &x_tx_family);
        send_tx_family(generate_node, &y_tx_family);
        propose_tx_family(generate_node, &x_tx_family);
        generate_node.submit_block(&blank(generate_node)); // move gap step
        generate_node.submit_block(&blank(generate_node));
        // debug_tx_family(generate_node, &y_tx_family);
        commit_tx_family(generate_node, &x_tx_family);
        generate_node.generate_blocks(1);
        // debug_tx_family(generate_node, &y_tx_family);
        assert_committed(generate_node, &x_tx_family);
        let delta_chain = (test_node.get_tip_block_number() + 1
            ..=generate_node.get_tip_block_number())
            .map(|number| generate_node.get_block_by_number(number))
            .collect::<Vec<_>>();

        // Put family y into pending-pool;
        // then test_node synchronizes delta chain.
        send_tx_family(test_node, &y_tx_family);
        delta_chain.iter().for_each(|block| {
            test_node.submit_block(block);
        });

        test_node.generate_blocks(window.farthest() as usize);
        // debug_tx_family(test_node, &y_tx_family);
        // debug_chain(test_node, 14);
    }
}

pub struct ProposalIDCollisionOnGapIrrelevant;

impl Spec for ProposalIDCollisionOnGapIrrelevant {
    crate::name!("proposal_id_collision_on_gap_irrelevant");

    crate::setup!(num_nodes: 2, connect_all: false);

    fn run(&self, net: &mut Net) {
        let generate_node = &net.nodes[0];
        let test_node = &net.nodes[1];
        let window = generate_node.consensus().tx_proposal_window();
        (0..=window.farthest() + 2).for_each(|_| {
            let block = generate_node.new_block(None, None, None);
            generate_node.submit_block(&block);
            test_node.submit_block(&block);
        });

        // x: x_tx_family
        // y: y_tx_family
        // x.a() is irrelevant with y.a(),
        // but x.a().proposal_id() == y.a().proposal_id()
        let (x_tx_family, y_tx_family) = irrelevant_collided_tx_families(generate_node);

        // Prepare: Generate delta chain: [propose(x)] -> [propose(y)] -> [] -> [commit(x)].
        //
        // After that, family y should be gaped state in the gap-pool.
        send_tx_family(generate_node, &x_tx_family);
        send_tx_family(generate_node, &y_tx_family);
        propose_tx_family(generate_node, &x_tx_family);
        propose_tx_family(generate_node, &y_tx_family);
        generate_node.submit_block(&blank(generate_node)); // move gap step
                                                           // debug_tx_family(generate_node, &y_tx_family);
        commit_tx_family(generate_node, &x_tx_family);
        generate_node.generate_blocks(1);
        // debug_tx_family(generate_node, &y_tx_family);
        assert_committed(generate_node, &x_tx_family);
        let delta_chain = (test_node.get_tip_block_number() + 1
            ..=generate_node.get_tip_block_number())
            .map(|number| generate_node.get_block_by_number(number))
            .collect::<Vec<_>>();

        // Put family y into pending-pool;
        // then test_node synchronizes delta chain.
        send_tx_family(test_node, &y_tx_family);
        delta_chain.iter().for_each(|block| {
            test_node.submit_block(block);
        });

        test_node.generate_blocks(window.farthest() as usize);
        // debug_tx_family(test_node, &y_tx_family);
        // debug_chain(test_node, 14);
    }
}

pub struct ProposalIDCollisionOnProposedIrrelevant;

impl Spec for ProposalIDCollisionOnProposedIrrelevant {
    crate::name!("proposal_id_collision_on_proposed_irrelevant");

    crate::setup!(num_nodes: 2, connect_all: false);

    fn run(&self, net: &mut Net) {
        let generate_node = &net.nodes[0];
        let test_node = &net.nodes[1];
        let window = generate_node.consensus().tx_proposal_window();
        (0..=window.farthest() + 2).for_each(|_| {
            let block = generate_node.new_block(None, None, None);
            generate_node.submit_block(&block);
            test_node.submit_block(&block);
        });

        // x: x_tx_family
        // y: y_tx_family
        // x.a() is irrelevant with y.a(),
        // but x.a().proposal_id() == y.a().proposal_id()
        let (x_tx_family, y_tx_family) = irrelevant_collided_tx_families(generate_node);

        // Prepare: Generate delta chain: [propose(x)] -> [] -> [propose(y)] -> [commit(x)].
        //
        // After that, family y is proposed state in the proposal-window.
        send_tx_family(generate_node, &x_tx_family);
        send_tx_family(generate_node, &y_tx_family);
        propose_tx_family(generate_node, &x_tx_family);
        generate_node.submit_block(&blank(generate_node)); // move gap step
        propose_tx_family(generate_node, &y_tx_family);
        // debug_tx_family(generate_node, &y_tx_family);
        commit_tx_family(generate_node, &x_tx_family);
        generate_node.generate_blocks(1);

        debug_tx_family(generate_node, &y_tx_family);
        assert_committed(generate_node, &x_tx_family);
        let delta_chain = (test_node.get_tip_block_number() + 1
            ..=generate_node.get_tip_block_number())
            .map(|number| generate_node.get_block_by_number(number))
            .collect::<Vec<_>>();

        // Put family y into pending-pool;
        // then test_node synchronizes delta chain.
        send_tx_family(test_node, &y_tx_family);
        delta_chain.iter().for_each(|block| {
            test_node.submit_block(block);
        });

        test_node.generate_blocks(window.farthest() as usize);
        debug_tx_family(test_node, &y_tx_family);
        debug_chain(test_node, 14);
    }
}

fn debug_tx_family(node: &Node, tx_family: &TxFamily) {
    println!();
    for i in 0..FAMILY_SIZE {
        let tx = tx_family.get(i);
        let status = node
            .rpc_client()
            .get_transaction(tx.hash())
            .map(|s| s.tx_status.status);
        println!(
            "TxFamily[{}] status: {:?}, hash: {}, actual_hash: {}, input_out_point: {}",
            i,
            status,
            tx.hash(),
            tx.data().calc_tx_hash(),
            tx.inputs().get(0).unwrap().previous_output().tx_hash(),
        );
    }
}

fn debug_chain(node: &Node, skip: BlockNumber) {
    for number in skip..=node.get_tip_block_number() {
        let block = node.get_block_by_number(number);
        println!("Block[{}]:", number);

        print!("\t\tProposals: [");
        for proposal in block.union_proposal_ids() {
            print!("{}, ", proposal);
        }
        println!("]");

        print!("\t\tCommitted: [");
        for committed in block.transactions().iter().skip(1) {
            print!("{}, ", committed.hash());
        }
        println!("]");
    }
}

// Construct a Byte32 with `[proposal_id, 0, 0, 0, ... 0]`
fn fake_hash(proposal_id: &ProposalShortId) -> Byte32 {
    let mut array = [0u8; 32];
    array[..10].copy_from_slice(&proposal_id.as_slice()[..10]);
    array.pack()
}

fn fake_hash_witness(proposal_id: &ProposalShortId) -> packed::Bytes {
    let fake_hash = fake_hash(proposal_id);
    fake_hash.as_bytes().pack()
}

fn is_fake_transaction(transaction: &TransactionView) -> bool {
    transaction.hash() != transaction.data().calc_tx_hash()
}

// Returns two tx_families `a_tx_family` and `b_tx_family` which meets:
//   * `a_tx_family` and `b_tx_family` are irrelevant and
//   * `a_tx_family.a().proposal_id() == b_tx_family.a().proposal_id()`
fn irrelevant_collided_tx_families(node: &Node) -> (TxFamily, TxFamily) {
    let txa = node.new_transaction(
        node.get_block_by_number(node.get_tip_block_number() - 1)
            .transaction(0)
            .unwrap()
            .hash(),
    );
    let txb = node
        .new_transaction_spend_tip_cellbase()
        .as_advanced_builder()
        .witness(fake_hash_witness(&txa.proposal_short_id()))
        .build()
        .fake_hash(fake_hash(&txa.proposal_short_id()));
    assert_ne!(txa.hash(), txb.hash());
    assert_eq!(txa.proposal_short_id(), txb.proposal_short_id());

    // NOTE: We construct a fake_hash_transaction by filling the fake_hash into the witness.
    // But we just wanna construct the b_tx_family.a() as fake_hash_transaction and preserve
    // the its descendants valid, so here get rid of the witnesses from b_tx_family.b(),
    // b_tx_family.c().
    let mut b_tx_family = TxFamily::init(txb);
    (1..FAMILY_SIZE).for_each(|i| {
        let tx = b_tx_family.get_mut(i);
        *tx = tx.as_advanced_builder().set_witnesses(vec![]).build();
    });
    (TxFamily::init(txa), b_tx_family)
}

// Returns two tx_families `a_tx_family` and `b_tx_family` which meets:
//   * `a_tx_family.a()` and `b_tx_family.a()` are cousin and
//   * `a_tx_family.a().proposal_id() == b_tx_family.a().proposal_id()`
fn cousin_collided_tx_families(node: &Node) -> (TxFamily, TxFamily) {
    let txa = node.new_transaction_spend_tip_cellbase();
    let output_data = Bytes::from(b"b0b".to_vec());
    let output = txa
        .output(0)
        .unwrap()
        .as_builder()
        .build_exact_capacity(Capacity::bytes(output_data.len()).unwrap())
        .unwrap();
    let txb = node
        .new_transaction_spend_tip_cellbase()
        .as_advanced_builder()
        .set_outputs_data(vec![output_data.pack()])
        .set_outputs(vec![output])
        .witness(fake_hash_witness(&txa.proposal_short_id()))
        .build()
        .fake_hash(fake_hash(&txa.proposal_short_id()));
    assert_ne!(txa.hash(), txb.hash());
    assert_eq!(txa.proposal_short_id(), txb.proposal_short_id());

    // NOTE: We construct a fake_hash_transaction by filling the fake_hash into the witness.
    // But we just wanna construct the b_tx_family.a() as fake_hash_transaction and preserve
    // the its descendants valid, so here get rid of the witnesses from b_tx_family.b(),
    // b_tx_family.c().
    let mut b_tx_family = TxFamily::init(txb);
    (1..FAMILY_SIZE).for_each(|i| {
        let tx = b_tx_family.get_mut(i);
        *tx = tx.as_advanced_builder().set_witnesses(vec![]).build();
    });
    (TxFamily::init(txa), b_tx_family)
}

fn send_tx_family(node: &Node, tx_family: &TxFamily) {
    (0..FAMILY_SIZE).for_each(|i| {
        let tx = tx_family.get(i);
        node.rpc_client().send_mock_transaction(tx.data().into());
    });
}

fn propose_tx_family(node: &Node, tx_family: &TxFamily) {
    let proposals: Vec<_> = (0..FAMILY_SIZE).map(|i| tx_family.get(i)).collect();
    let block = propose(node, &proposals.as_ref());
    node.rpc_client().send_mock_block(block.data().into());
}

fn commit_tx_family(node: &Node, tx_family: &TxFamily) {
    let committed: Vec<_> = (0..FAMILY_SIZE).map(|i| tx_family.get(i)).collect();
    let block = commit(node, &committed.as_ref());
    node.rpc_client().send_mock_block(block.data().into());
}

fn assert_committed(node: &Node, tx_family: &TxFamily) {
    let uncommitted: Vec<_> = (0..FAMILY_SIZE)
        .filter_map(|i| {
            let tx_hash = tx_family.get(i).hash();
            let tx_status = node
                .rpc_client()
                .get_transaction(tx_hash.clone())
                .expect(&format!(
                    "submitted transaction should be accessed {}",
                    tx_hash
                ));
            if tx_status.tx_status.status == TxStatus::committed(h256!("0x0")).status {
                None
            } else {
                Some(i)
            }
        })
        .collect();
    assert!(
        uncommitted.is_empty(),
        "uncommitted indices: {:?}, hashes: {:?}",
        uncommitted,
        uncommitted
            .iter()
            .map(|i| tx_family.get(*i))
            .collect::<Vec<_>>(),
    );
}
