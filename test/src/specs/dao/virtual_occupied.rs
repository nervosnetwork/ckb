use super::*;
use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_chain_spec::{ChainSpec, IssuedCell};
use ckb_dao_utils::extract_dao_data;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{bytes::Bytes, core::Capacity, prelude::*, virtual_occupied::gen_occupied_data};
use faster_hex::hex_encode;

const VIRTUAL_OCCUPIED: Capacity = Capacity::shannons(10_000_000_000_000_000);
const CELLBASE_USED_BYTES: usize = 41;

pub struct DAOWithVirtualOccupied;

impl Spec for DAOWithVirtualOccupied {
    crate::name!("dao_with_virtual_occupied");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        // try deposit then withdraw dao
        node0.generate_blocks(2);
        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        let transaction = withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
        node0.generate_blocks(20);
        let tx_hash = node0
            .rpc_client()
            .send_transaction(transaction.data().into());
        node0.generate_blocks(3);
        let tx_status = node0
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {:#x}",
            tx_hash
        );
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            let virtual_occupied_cell = issue_virtual_occupied_cell(VIRTUAL_OCCUPIED);
            spec_config.genesis.issued_cells.push(virtual_occupied_cell);
        })
    }
}

pub struct SpendVirtualOccupiedCell;

impl Spec for SpendVirtualOccupiedCell {
    crate::name!("spend_virtual_occupied_cell");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        // check genesis blocks dao
        let genesis = node0.get_block_by_number(0);
        let (_ar, _c, u) = extract_dao_data(genesis.header().dao()).expect("extract dao");
        // u - used capacity should includes virtual occupied
        assert!(u > VIRTUAL_OCCUPIED);

        // Build tx to spent virtual occupied capacity
        let cellbase = &genesis.transactions()[0];
        let virutal_occupied_input = CellInput::new(
            OutPoint::new(
                cellbase.hash().unpack(),
                (cellbase.outputs().len() - 1) as u32,
            ),
            0,
        );
        let always_dep = CellDep::new_builder()
            .out_point(OutPoint::new(
                cellbase.hash().unpack(),
                SYSTEM_CELL_ALWAYS_SUCCESS_INDEX,
            ))
            .build();
        let output = CellOutput::new_builder()
            .capacity(VIRTUAL_OCCUPIED.pack())
            .lock(always_success_cell().2.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .cell_deps(vec![always_dep])
            .input(virutal_occupied_input)
            .output(output)
            .output_data(Bytes::new().pack())
            .build();

        node0.generate_blocks(1);
        let tx_hash = node0
            .rpc_client()
            .send_transaction(transaction.data().into());
        node0.generate_blocks(3);
        // cellbase occupied capacity
        let cellbase_used_capacity = Capacity::bytes(CELLBASE_USED_BYTES * 4).unwrap();
        let tx_status = node0
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {:#x}",
            tx_hash
        );
        let tip = node0.get_tip_block();
        // check tip dao
        let (_ar, _c, new_u) = extract_dao_data(tip.header().dao()).expect("extract dao");
        assert_eq!(
            Ok(new_u),
            u.safe_sub(VIRTUAL_OCCUPIED)
                .and_then(|c| c.safe_add(cellbase_used_capacity))
        );
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            let virtual_occupied_cell = issue_virtual_occupied_cell(VIRTUAL_OCCUPIED);
            spec_config.genesis.issued_cells.push(virtual_occupied_cell);
        })
    }
}

fn issue_virtual_occupied_cell(capacity: Capacity) -> IssuedCell {
    let data = {
        let data = gen_occupied_data(capacity);
        let mut hex_data = vec![0; data.len() * 2 + 2];
        hex_data[0] = b'0';
        hex_data[1] = b'x';
        hex_encode(&data, &mut hex_data[2..]).expect("hex encode");
        String::from_utf8(hex_data).expect("to string")
    };
    let lock = always_success_cell().2.clone().into();
    IssuedCell {
        capacity: VIRTUAL_OCCUPIED,
        data,
        lock,
    }
}
