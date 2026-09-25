use crate::util::transaction::{relay_tx, send_tx};
use crate::utils::{build_compact_block, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, Cycle, ScriptHashType, TransactionView, capacity_bytes},
    packed::{
        BlockProposal, Byte32, CellDep, CellInput, CellOutput, OutPoint, RelayMessage,
        RelayMessageUnion, RelayTransactionHashes, Script,
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
        config.tx_pool.max_tx_verify_time_ms = 1;
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
        let (funding, tx, cycles) = fund_budget_workload(node);
        let rpc = node.rpc_client();
        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::RelayV3],
        );
        net.connect(node);
        relay_tx(&net, node, tx.clone(), cycles);

        assert_relay_retry(&net, node, &tx.hash());

        // Exercise the real proposal response, which carries no declared cycles
        // or peer metadata into the verification queue.
        let block = node
            .new_block_builder(None, None, None)
            .proposal(tx.proposal_short_id())
            .build();
        net.send(node, SupportProtocols::RelayV3, build_compact_block(&block));
        assert!(net.should_receive(node, |data| {
            RelayMessage::from_slice(data).is_ok_and(|message| {
                matches!(
                    message.to_enum(),
                    RelayMessageUnion::GetBlockProposal(request)
                        if request.proposals().into_iter().any(|id| id == tx.proposal_short_id())
                )
            })
        }));
        assert!(wait_until(10, || node.get_tip_block() == block));
        let response = RelayMessage::new_builder()
            .set(
                BlockProposal::new_builder()
                    .transactions(vec![tx.data()])
                    .build(),
            )
            .build();
        net.send(node, SupportProtocols::RelayV3, response.as_bytes());
        // Proposal ingress does not mark an unverified body known. Observe its
        // committed refusal; the unanswered relay request remains independent.
        let hash = format!("\"hash\":\"{:x}\"", tx.hash());
        assert!(wait_until(10, || {
            node.access_log(|line| {
                line.contains(&hash)
                    && line.contains("\"event\":\"committed_rejection\"")
                    && line.contains("\"source\":\"proposal\"")
                    && line.contains("\"reason\":\"ExcessiveVerifyTime\"")
            })
            .unwrap()
        }));
        assert_eq!(
            rpc.get_transaction(tx.hash()).tx_status.status,
            ckb_jsonrpc_types::Status::Unknown
        );

        // Fulfil the earlier relay retry request while the parent is missing.
        // Restoring it must wake waiting work without losing its network budget.
        assert!(rpc.remove_transaction(funding.hash()));
        send_tx(&net, node, tx.clone(), cycles);
        assert!(wait_until(10, || rpc.tx_pool_info().orphan.value() == 1));
        node.submit_transaction(&funding);
        assert!(wait_until(10, || rpc.tx_pool_info().orphan.value() == 0));
        assert_relay_retry(&net, node, &tx.hash());

        assert_eq!(node.submit_transaction(&tx), tx.hash());
        let accepted = rpc.get_transaction(tx.hash());
        assert_eq!(
            accepted.tx_status.status,
            ckb_jsonrpc_types::Status::Pending
        );
        assert_eq!(accepted.cycles.unwrap().value(), cycles);
        // The compact block already proposed the transaction; mine the gap
        // before waiting for it in the commit section.
        node.mine(node.consensus().tx_proposal_window().closest() - 1);
        node.mine_with_blocking(|template| {
            !template
                .transactions
                .iter()
                .any(|entry| entry.hash == tx.hash().into())
        });
        assert_eq!(
            rpc.get_transaction(tx.hash()).tx_status.status,
            ckb_jsonrpc_types::Status::Committed
        );
    }
}

// Prepare a consensus-valid workload without warming the pool proof cache.
fn fund_budget_workload(node: &Node) -> (TransactionView, TransactionView, Cycle) {
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
    let funding = node
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
        .set_outputs(vec![
            CellOutput::new_builder()
                .capacity(capacity_bytes!(6_500))
                .lock(node.always_success_script())
                .build(),
        ])
        .build();
    node.submit_transaction(&funding);
    let tx = funding
        .as_advanced_builder()
        .set_inputs(vec![CellInput::new(OutPoint::new(funding.hash(), 0), 0)])
        .cell_dep(dep)
        .set_outputs(outputs)
        .set_outputs_data(vec![Bytes::new().pack(); 64])
        .build();
    // Estimate against committed inputs without warming the pool's proof
    // cache. These fixtures do not inspect inputs: both forms run the same
    // always-success lock and 64 current-cycles type groups.
    let estimate = tx
        .as_advanced_builder()
        .set_inputs(funding.inputs().into_iter().collect())
        .build();
    let cycles = rpc.estimate_cycles(estimate.data().into()).cycles.value();
    (funding, tx, cycles)
}

fn assert_relay_retry(net: &Net, node: &Node, tx_hash: &Byte32) {
    let rpc = node.rpc_client();
    let announcement = RelayMessage::new_builder()
        .set(
            RelayTransactionHashes::new_builder()
                .tx_hashes(vec![tx_hash.clone()])
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
                            if request.tx_hashes().into_iter().any(|hash| hash == *tx_hash)
                    )
                })
        }),
        "a time-limited transaction must be requestable again"
    );
    assert_eq!(
        rpc.get_transaction(tx_hash.clone()).tx_status.status,
        ckb_jsonrpc_types::Status::Unknown,
        "time exhaustion must not become a persistent rejection"
    );
    assert_eq!(rpc.get_peers().len(), 1);
    assert!(rpc.get_banned_addresses().is_empty());
}
