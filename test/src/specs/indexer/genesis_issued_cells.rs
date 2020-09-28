use crate::utils::wait_until;
use crate::{Node, Spec};
use ckb_chain_spec::IssuedCell;
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, ScriptHashType},
    h256,
    packed::Script,
    prelude::*,
    H256,
};
use log::info;

pub struct GenesisIssuedCells;

impl Spec for GenesisIssuedCells {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        let lock_hash = Script::new_builder()
            .args(Bytes::from(vec![1, 2]).pack())
            .code_hash(
                // The second output's type_id script hash
                h256!("0x82d76d1b75fe2fd9a27dfbaa65a039221a380d76c926f378d3f81cf3e7e13f2e").pack(),
            )
            .hash_type(ScriptHashType::Type.into())
            .build()
            .calc_script_hash();
        info!("{}", lock_hash);
        let rpc_client = node0.rpc_client();

        info!("Should return live cells and cell transactions of genesis issued cells");
        rpc_client.index_lock_hash(lock_hash.clone(), Some(0));
        let result = wait_until(5, || {
            let live_cells = rpc_client.get_live_cells_by_lock_hash(lock_hash.clone(), 0, 20, None);
            let cell_transactions =
                rpc_client.get_transactions_by_lock_hash(lock_hash.clone(), 0, 20, None);
            live_cells.len() == 1 && cell_transactions.len() == 1
        });
        if !result {
            panic!("Wrong indexer store index data");
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.genesis.issued_cells = vec![IssuedCell {
            capacity: capacity_bytes!(5_000),
            lock: Script::new_builder()
                .args(Bytes::from(vec![1, 2]).pack())
                .code_hash(
                    // The second output's type_id script hash
                    h256!("0x82d76d1b75fe2fd9a27dfbaa65a039221a380d76c926f378d3f81cf3e7e13f2e")
                        .pack(),
                )
                .hash_type(ScriptHashType::Type.into())
                .build()
                .into(),
        }];
    }
}
