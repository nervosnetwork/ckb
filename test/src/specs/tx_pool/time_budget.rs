use crate::util::transaction::relay_tx;
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, ScriptHashType, capacity_bytes},
    packed::{
        CellDep, CellInput, CellOutput, OutPoint, RelayMessage, RelayMessageUnion,
        RelayTransactionHashes, Script,
    },
    prelude::*,
};
use std::time::Duration;

const PROGRAM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../script/testdata/current_cycles"
);

pub struct NetworkVerificationTimeBudget;

impl Spec for NetworkVerificationTimeBudget {
    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_time_ms = std::num::NonZeroU32::MIN;
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.genesis.system_cells.push(ckb_chain_spec::SystemCell {
            create_type_id: false,
            capacity: None,
            file: ckb_resource::Resource::file_system(PROGRAM.into()),
        });
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.mine_until_out_bootstrap_period();
        // Four cellbase rewards fund the 64 output cells.
        node.mine(4);
        let tip = node.get_tip_block_number();
        let rpc = node.rpc_client();
        let code_hash = CellOutput::calc_data_hash(&Bytes::from(std::fs::read(PROGRAM).unwrap()));
        let genesis = &node.consensus().genesis_block().transactions()[0];
        let index = genesis
            .outputs_data()
            .into_iter()
            .position(|data| CellOutput::calc_data_hash(&data.raw_data()) == code_hash)
            .expect("the workload is a genesis system cell");
        let dep = CellDep::new_builder()
            .out_point(OutPoint::new(genesis.hash(), index as u32))
            .build();

        // Distinct groups each execute the existing 4,096-syscall fixture.
        // Together they comfortably exceed 1 ms while remaining consensus-valid.
        let outputs = (0u8..64)
            .map(|group| {
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(100))
                    .lock(node.always_success_script())
                    .type_(Some(
                        Script::new_builder()
                            .code_hash(code_hash.clone())
                            .hash_type(ScriptHashType::Data1)
                            .args(Bytes::from(vec![group]))
                            .build(),
                    ))
                    .build()
            })
            .collect();
        let tx = node
            .new_transaction_spend_tip_cellbase()
            .as_advanced_builder()
            .set_inputs(
                (tip - 3..=tip)
                    .map(|height| {
                        let cellbase = node.get_block_by_number(height).transactions()[0].hash();
                        CellInput::new(OutPoint::new(cellbase, 0), 0)
                    })
                    .collect(),
            )
            .cell_dep(dep)
            .set_outputs(outputs)
            .set_outputs_data(vec![Bytes::new().pack(); 64])
            .build();
        // Estimation verifies scripts without populating the pool's script cache.
        let cycles = rpc.estimate_cycles(tx.data().into()).cycles.value();
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node);
        relay_tx(&net, node, tx.clone(), cycles);

        let announcement = RelayMessage::new_builder()
            .set(
                RelayTransactionHashes::new_builder()
                    .tx_hashes(vec![tx.hash()])
                    .build(),
            )
            .build();
        // A fresh request for the same hash acknowledges removal of the known
        // marker after refusal. An empty queue alone could precede delivery.
        assert!(
            wait_until(15, || {
                net.send(node, SupportProtocols::RelayV3, announcement.as_bytes());
                net.receive_timeout(node, Duration::from_millis(200))
                    .ok()
                    .and_then(|(_, _, data)| RelayMessage::from_slice(&data).ok())
                    .is_some_and(|message| {
                        matches!(
                            message.to_enum(),
                            RelayMessageUnion::GetRelayTransactions(request)
                                if request.tx_hashes().into_iter().any(|hash| hash == tx.hash())
                        )
                    })
            }),
            "a time-limited transaction must be requestable again"
        );
        assert_eq!(
            rpc.get_transaction(tx.hash()).tx_status.status,
            ckb_jsonrpc_types::Status::Unknown,
            "time exhaustion must not become a persistent rejection"
        );
        assert_eq!(rpc.get_peers().len(), 1);
        assert!(rpc.get_banned_addresses().is_empty());

        assert_eq!(node.submit_transaction(&tx), tx.hash());
        assert_eq!(
            rpc.get_transaction(tx.hash()).tx_status.status,
            ckb_jsonrpc_types::Status::Pending
        );
        node.mine_until_transaction_confirm(&tx.hash());
    }
}
