use crate::{Net, Node, Spec};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::extract_dao_data;
use ckb_jsonrpc_types::TransactionPoint;
use ckb_types::core::{BlockNumber, BlockView, Capacity, HeaderView, TransactionView};
use ckb_types::packed::Bytes;
use ckb_types::prelude::*;
use ckb_types::H256;
use rand::prelude::ThreadRng;
use rand::{seq::SliceRandom, thread_rng, Rng};
use std::collections::HashMap;

pub struct PrimaryRewardOfChainsWithoutAnyUncles;

impl Spec for PrimaryRewardOfChainsWithoutAnyUncles {
    crate::name!("primary_reward_of_chains_without_any_uncles");

    crate::setup!(num_nodes: 2);

    // Case: Different chains without any uncles, should issue the same primary_block_reward
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let mut rng = thread_rng();

        let consensus = node0.consensus();
        let genesis_epoch_length = consensus.genesis_epoch_ext().length();
        let farthest = consensus.tx_proposal_window().farthest();

        for _ in 0..genesis_epoch_length + farthest + 20 {
            mine_blocks_on_tip(node0, 0);
            mine_blocks_on_tip(node1, 0);
            spend_transaction_randomly(node0, &mut rng);
            spend_transaction_randomly(node1, &mut rng);
            checking_same_primary_reward(node0, node1);
        }
    }
}

pub struct PrimaryRewardOfChainsWithTheSameNumberOfUncles;

impl Spec for PrimaryRewardOfChainsWithTheSameNumberOfUncles {
    crate::name!("primary_reward_of_chains_with_the_same_number_of_uncles");

    crate::setup!(num_nodes: 2);

    // Case: Different chains with the same number of uncles, should issue the same primary_block_reward
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let mut rng = thread_rng();

        let consensus = node0.consensus();
        let genesis_epoch_length = consensus.genesis_epoch_ext().length();
        let farthest = consensus.tx_proposal_window().farthest();

        let uncles_count = rng.gen_range(genesis_epoch_length / 5, genesis_epoch_length) as usize;
        let uncles0 = distribute_uncles_randomly(consensus, uncles_count, &mut rng);
        let uncles1 = distribute_uncles_randomly(consensus, uncles_count, &mut rng);

        for block_number in 0..genesis_epoch_length + farthest + 20 {
            mine_blocks_on_tip(node0, *uncles0.get(&block_number).unwrap_or(&0));
            mine_blocks_on_tip(node1, *uncles1.get(&block_number).unwrap_or(&0));
            spend_transaction_randomly(node0, &mut rng);
            spend_transaction_randomly(node1, &mut rng);
            checking_same_primary_reward(node0, node1);
        }
    }
}

pub struct PrimaryRewardOfChainsWithTheSameNumberOfUncles2;

impl Spec for PrimaryRewardOfChainsWithTheSameNumberOfUncles2 {
    crate::name!("primary_reward_of_chains_with_the_same_number_of_uncles2");

    crate::setup!(num_nodes: 2);

    // Case: Different chains with the same number of uncles, should issue the same primary_block_reward
    //
    // The difference between `PrimaryRewardOfChainsWithTheSameNumberOfUncles` is that the uncles of
    // `chain1` are all with the same height
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let node1 = &net.nodes[1];
        let mut rng = thread_rng();

        let consensus = node0.consensus();
        let genesis_epoch_length = consensus.genesis_epoch_ext().length();
        let farthest = consensus.tx_proposal_window().farthest();

        let uncles_count = rng.gen_range(genesis_epoch_length / 5, genesis_epoch_length) as usize;
        let uncles0 = distribute_uncles_randomly(consensus, uncles_count, &mut rng);
        let uncles1 = {
            let mut uncles1 = HashMap::new();
            uncles1.insert(10, uncles_count);
            uncles1
        };

        for block_number in 0..genesis_epoch_length + farthest + 20 {
            mine_blocks_on_tip(node0, *uncles0.get(&block_number).unwrap_or(&0));
            mine_blocks_on_tip(node1, *uncles1.get(&block_number).unwrap_or(&0));
            spend_transaction_randomly(node0, &mut rng);
            spend_transaction_randomly(node1, &mut rng);
            checking_same_primary_reward(node0, node1);
        }
    }
}

pub struct TotalIssuedOccupiedCapacities;

impl Spec for TotalIssuedOccupiedCapacities {
    crate::name!("total_issued_occupied_capacities");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let mut rng = thread_rng();

        let consensus = node0.consensus();
        let genesis_epoch_length = consensus.genesis_epoch_ext().length();
        let farthest = consensus.tx_proposal_window().farthest();

        // Check genesis dao field
        {
            let (_remote_ar, remote_c, remote_u) = remote_tip_dao(node0);
            let (local_c, local_u) = expected_tip_dao(node0);
            assert_eq!(
                (remote_c, remote_u),
                (local_c, local_u),
                "unmatched genesis dao field"
            );
        }

        // Check dao field after mining an empty-blocks
        {
            node0.generate_block();
            let (_remote_ar, remote_c, remote_u) = remote_tip_dao(node0);
            let (local_c, local_u) = expected_tip_dao(node0);
            assert_eq!(
                (remote_c, remote_u),
                (local_c, local_u),
                "unmatched tip dao field"
            );
        }

        // Check dao field after mining some empty-blocks
        {
            node0.generate_blocks((genesis_epoch_length + farthest + 20) as usize);
            let (_remote_ar, remote_c, remote_u) = remote_tip_dao(node0);
            let (local_c, local_u) = expected_tip_dao(node0);
            assert_eq!(
                (remote_c, remote_u),
                (local_c, local_u),
                "unmatched tip dao field"
            );
        }

        // Check dao field after mining some non-empty-blocks
        {
            for _ in 0..genesis_epoch_length + farthest + 20 {
                node0.generate_block();
                spend_transaction_randomly(node0, &mut rng);
            }
            let (_remote_ar, remote_c, remote_u) = remote_tip_dao(node0);
            let (local_c, local_u) = expected_tip_dao(node0);
            assert_eq!(
                (remote_c, remote_u),
                (local_c, local_u),
                "unmatched tip dao field"
            );
        }
    }
}

fn checking_same_primary_reward(node0: &Node, node1: &Node) {
    let hash0 = node0.rpc_client().get_tip_header().hash;
    let hash1 = node1.rpc_client().get_tip_header().hash;
    let reward0 = node0
        .rpc_client()
        .get_cellbase_output_capacity_details(hash0);
    let reward1 = node1
        .rpc_client()
        .get_cellbase_output_capacity_details(hash1);
    let epoch0 = node0.rpc_client().get_current_epoch();
    let epoch1 = node1.rpc_client().get_current_epoch();
    let cellbase_capacity0: Capacity = node0
        .get_tip_block()
        .output(0, 0)
        .expect("cellbase output exist")
        .capacity()
        .unpack();
    let cellbase_capacity1: Capacity = node1
        .get_tip_block()
        .output(0, 0)
        .expect("cellbase output exist")
        .capacity()
        .unpack();

    assert_eq!(
        reward0.primary, reward1.primary,
        "different chains without any uncles should issue the same primary_block_reward",
    );
    assert_eq!(
        epoch0, epoch1,
        "different chains without any uncles should have the same epoch_ext"
    );
    assert_eq!(
        cellbase_capacity0, reward0.total.0,
        "get_cellbase_output_capacity_details.total != cellbase.output[0].capacity",
    );
    assert_eq!(
        cellbase_capacity1, reward1.total.0,
        "get_cellbase_output_capacity_details.total != cellbase.output[0].capacity",
    );
}

fn spend_transaction_randomly(node: &Node, rng: &mut ThreadRng) {
    let seed: u8 = rng.gen_range(1, 5);
    if seed % 3 == 0 {
        let output_data = Bytes::new_builder()
            .extend((0..seed).collect::<Vec<_>>())
            .build();
        let transaction = node
            .new_transaction_spend_tip_cellbase()
            .as_advanced_builder()
            .set_outputs_data(vec![output_data])
            .build();
        node.submit_transaction(&transaction);
    }
}

fn distribute_uncles_randomly(
    consensus: &Consensus,
    count: usize,
    rng: &mut ThreadRng,
) -> HashMap<BlockNumber, usize> {
    let genesis_epoch_start = 1;
    let genesis_epoch_end = consensus.genesis_epoch_ext().length();

    let mut seq = (genesis_epoch_start..genesis_epoch_end - 1).collect::<Vec<_>>();
    (0..consensus.max_uncles_num).for_each(|_| seq.extend(seq.clone()));
    seq.shuffle(rng);
    seq.truncate(count);

    let mut uncles = HashMap::new();
    for uncle_number in seq {
        uncles
            .entry(uncle_number)
            .and_modify(|c| *c += 1)
            .or_insert(1);
    }

    uncles
}

// Mine `uncles_count` + 1 blocks, so there will be `uncles_count` uncles and only 1 block on
// main chain
fn mine_blocks_on_tip(node: &Node, uncles_count: usize) {
    let template = node.new_block(None, None, None);
    for i in 0..=uncles_count {
        let block_timestamp = template.timestamp() + i as u64;
        let block = template
            .clone()
            .as_advanced_builder()
            .timestamp(block_timestamp.pack())
            .build();
        node.submit_block(&block.data());
    }
}

// Return all the transactions on the main chain
fn all_transactions(node: &Node) -> HashMap<H256, TransactionView> {
    let mut transactions = HashMap::new();
    (0..=node.get_tip_block_number()).for_each(|i| {
        let block: BlockView = node.get_block_by_number(i);
        for transaction in block.transactions() {
            transactions.insert(transaction.hash().unpack(), transaction);
        }
    });
    transactions
}

// Return the current live cells set
fn all_live_cells(node: &Node) -> Vec<TransactionPoint> {
    let lock_hash = node.always_success_script().calc_script_hash();
    let mut live_cells = Vec::new();

    for page in 0..100_000_000 {
        let cells =
            node.rpc_client()
                .get_live_cells_by_lock_hash(lock_hash.clone(), page, 50, None);
        if cells.is_empty() {
            break;
        }

        live_cells.extend(cells.into_iter().map(|c| c.created_by));
    }

    live_cells
}

// Get the actual (ar, total-issued-capacities, total-occupied-capacities) from the tip header
fn remote_tip_dao(node: &Node) -> (u64, Capacity, Capacity) {
    let tip_header: HeaderView = node.rpc_client().get_tip_header().into();
    extract_dao_data(tip_header.dao()).unwrap()
}

// Manually sum the expected (ar, total-issued-capacities, total-occupied-capacities)
//
// c = c + SUM([cell.capacity() for cell in live_cell_set])
// u = u + SUM([cell.occupied_capacity() for cell in live_cell_set])
//
// NOTE: Here assumes that all the live cells, expect genesis cells, are always-success cells.
fn expected_tip_dao(node: &Node) -> (Capacity, Capacity) {
    let (mut c, mut u) = non_always_success_dao(node);

    let transactions = all_transactions(node);
    let live_cells = all_live_cells(node);
    for cell in live_cells {
        let TransactionPoint { tx_hash, index, .. } = cell;
        let transaction = transactions.get(&tx_hash).expect("get transaction");
        let (output, data) = transaction
            .output_with_data(index.0 as usize)
            .expect("get output");
        let output_capacity: Capacity = output.capacity().unpack();
        let data_capacity = Capacity::bytes(data.len()).unwrap();
        c = c.safe_add(output_capacity).expect("sum issued capacities");
        u = u
            .safe_add(output.occupied_capacity(data_capacity).unwrap())
            .expect("sum occupied capacities");
    }

    (c, u)
}

// Sum the (ar, total-issued-capacities, total-occupied-capacities) of the non-always-success cells
// within the genesis block.
//
// NOTE: Here assumes that non-always-success cells exist only in genesis block
fn non_always_success_dao(node: &Node) -> (Capacity, Capacity) {
    let (mut c, mut u) = (Capacity::zero(), Capacity::zero());
    let genesis = node.get_block_by_number(0);
    let transactions: HashMap<H256, TransactionView> = genesis
        .transactions()
        .into_iter()
        .map(|transaction| (transaction.hash().unpack(), transaction))
        .collect();

    for (tx_index, transaction) in genesis.transactions().into_iter().enumerate() {
        if tx_index != 0 {
            for input_out_point in transaction.input_pts_iter() {
                let input_transaction = transactions
                    .get(&input_out_point.tx_hash().unpack())
                    .expect("get input transaction");
                let (input_cell, input_data) = input_transaction
                    .output_with_data(input_out_point.index().unpack())
                    .expect("get input cell");
                let input_capacity: Capacity = input_cell.capacity().unpack();
                let input_occupied_capacity: Capacity = input_cell
                    .occupied_capacity(Capacity::bytes(input_data.len()).unwrap())
                    .unwrap();
                c = c
                    .safe_sub(input_capacity)
                    .expect("sub spent issued capacities");
                u = u
                    .safe_sub(input_occupied_capacity)
                    .expect("sub spent occupied capacities");
            }
        }

        for (output, data) in transaction.outputs_with_data_iter() {
            if output.lock() != node.always_success_script() {
                let output_capacity: Capacity = output.capacity().unpack();
                let data_capacity = Capacity::bytes(data.len()).unwrap();
                c = c.safe_add(output_capacity).expect("sum issued capacities");
                u = u
                    .safe_add(output.occupied_capacity(data_capacity).unwrap())
                    .expect("sum occupied capacities");
            }
        }
    }

    (c, u)
}
