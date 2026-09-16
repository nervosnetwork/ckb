use crate::{
    Node, Spec,
    util::{
        cell::{as_inputs, gen_spendable},
        transaction::always_success_transaction,
    },
};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::{
    H256,
    core::{Capacity, FeeRate, TransactionBuilder, cell::CellMetaBuilder},
    packed::{Block, CellOutput, OutPoint},
    prelude::*,
};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Templates retain valid commits while admissions refresh their proposals.
/// Each observation measures an actual RPC, including its queueing and transport.
pub struct TemplatesDuringAdmission;

fn observe(node: &Node, phase: &str, received: &AtomicUsize) -> BlockTemplate {
    let before = received.load(Ordering::Acquire);
    let started = Instant::now();
    let result = node
        .rpc_client()
        .inner()
        .get_block_template(None, None, None);
    let elapsed = started.elapsed();
    println!(
        "TEMPLATE_RPC {}",
        serde_json::json!({
            "phase": phase, "elapsed_ns": elapsed.as_nanos(),
            "received_before": before, "received_after": received.load(Ordering::Acquire),
            "result": result.as_ref().map(|template| serde_json::json!({
                "work_id": template.work_id, "transactions": template.transactions.len(),
                "proposals": template.proposals.len(), "parent_hash": template.parent_hash,
            })).map_err(ToString::to_string),
        })
    );
    result.expect("template RPC must respond during admission")
}

fn commits(template: &BlockTemplate) -> BTreeSet<H256> {
    template
        .transactions
        .iter()
        .map(|tx| tx.hash.clone())
        .collect()
}

impl Spec for TemplatesDuringAdmission {
    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.tx_pool.min_fee_rate = FeeRate::zero();
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        const COUNT: usize = 512;
        let node = &nodes[0];
        // Split a bounded set of cellbases once; mining one block per measured
        // transaction would dominate this regression's setup.
        let cells = gen_spendable(node, 32);
        let capacity = cells
            .iter()
            .map(|cell| cell.capacity().as_u64())
            .sum::<u64>();
        let output = CellOutput::new_builder()
            .capacity(Capacity::shannons(capacity / COUNT as u64))
            .lock(node.always_success_script())
            .build();
        let funding = TransactionBuilder::default()
            .inputs(as_inputs(&cells))
            .outputs(vec![output; COUNT])
            .outputs_data(vec![Default::default(); COUNT])
            .cell_dep(node.always_success_cell_dep())
            .build();
        node.submit_transaction(&funding);
        node.mine_until_transaction_confirm(&funding.hash());
        let transactions: Vec<_> = (0..COUNT)
            .map(|index| {
                let cell = CellMetaBuilder::from_cell_output(
                    funding.output(index).unwrap(),
                    Default::default(),
                )
                .out_point(OutPoint::new(funding.hash(), index as u32))
                .build();
                always_success_transaction(node, &cell)
            })
            .collect();
        println!(
            "TEMPLATE_FIXTURE {}",
            serde_json::json!({
                "transaction_hashes": transactions.iter().map(|tx| H256::from(tx.hash())).collect::<Vec<_>>(),
                "initial_proposed": COUNT / 2, "later_pending": COUNT / 2,
            })
        );
        for tx in &transactions[..COUNT / 2] {
            node.submit_transaction(tx);
        }
        let proposal =
            node.new_block_with_blocking(|template| template.proposals.len() != COUNT / 2);
        node.submit_block(&proposal);
        node.mine(node.consensus().tx_proposal_window().closest() - 1);
        node.new_block_with_blocking(|template| template.transactions.len() != COUNT / 2);

        let received = AtomicUsize::new(0);
        let mut expected: BTreeSet<H256> = transactions[..COUNT / 2]
            .iter()
            .map(|tx| tx.hash().into())
            .collect();
        for _ in 0..64 {
            assert_eq!(commits(&observe(node, "warm", &received)), expected);
        }

        assert!(node.remove_transaction(transactions[0].hash()));
        expected.remove(&transactions[0].hash().into());
        // Removing a selected owner invalidates the old template immediately.
        assert_eq!(
            commits(&observe(node, "selected_owner_removed", &received)),
            expected
        );

        let admitted = Instant::now();
        thread::scope(|scope| {
            let producer = scope.spawn(|| {
                for tx in &transactions[COUNT / 2..] {
                    node.submit_transaction(tx);
                    received.fetch_add(1, Ordering::Release);
                }
            });
            while !producer.is_finished() {
                assert_eq!(commits(&observe(node, "admitting", &received)), expected);
                assert!(
                    admitted.elapsed() < Duration::from_secs(30),
                    "admission did not finish"
                );
            }
            producer.join().unwrap();
        });
        let admission_ns = admitted.elapsed().as_nanos();
        let proposals: BTreeSet<_> = transactions[COUNT / 2..]
            .iter()
            .map(|tx| ckb_jsonrpc_types::ProposalShortId::from(tx.proposal_short_id()).0)
            .collect();
        let template = loop {
            let template = observe(node, "awaiting_publication", &received);
            assert_eq!(commits(&template), expected);
            if template
                .proposals
                .iter()
                .map(|id| id.0)
                .collect::<BTreeSet<_>>()
                == proposals
            {
                break template;
            }
            assert!(
                admitted.elapsed() < Duration::from_secs(30),
                "admitted proposals did not become visible"
            );
            thread::sleep(Duration::from_millis(10));
        };
        println!(
            "TEMPLATE_PUBLICATION {}",
            serde_json::json!({
                "admitted_transactions": received.load(Ordering::Acquire),
                "admission_ns": admission_ns, "admission_to_visible_ns": admitted.elapsed().as_nanos(),
            })
        );
        assert_eq!(received.load(Ordering::Acquire), COUNT / 2);
        node.assert_tx_pool_size((COUNT / 2) as u64, (COUNT / 2 - 1) as u64);
        node.submit_block(&Block::from(template).into_view());
    }
}
