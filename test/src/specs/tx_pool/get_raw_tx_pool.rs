use super::utils::get_pool_entries;
use crate::{Node, Spec};
use ckb_jsonrpc_types::RawTxPool;
use ckb_logger::info;
use ckb_types::H256;
use std::collections::BTreeSet;

pub struct GetRawTxPool;

impl Spec for GetRawTxPool {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();

        info!("Generate 6 txs on node0");
        let mut txs_hash = vec![node0.generate_transaction()];

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(txs_hash.last().unwrap().clone());
            txs_hash.push(node0.rpc_client().send_transaction(tx.data().into()));
        });

        let mut pending: Vec<H256> = txs_hash.iter().map(Into::<H256>::into).collect();
        pending.sort();
        for proposed in [false, true] {
            if proposed {
                let height =
                    node0.mine_with_blocking(|template| template.proposals.len() != txs_hash.len());
                node0.mine_with_blocking(|template| template.number.value() != height + 1);
                node0.wait_for_tx_pool();
            }
            let (expected_pending, expected_proposed) = if proposed {
                (&[][..], pending.as_slice())
            } else {
                (pending.as_slice(), &[][..])
            };
            for verbosity in [None, Some(false)] {
                let RawTxPool::Ids(mut ids) = node0.rpc_client().get_raw_tx_pool(verbosity) else {
                    panic!("non-verbose pool query returned entries");
                };
                ids.pending.sort();
                ids.proposed.sort();
                assert_eq!(ids.pending, expected_pending);
                assert_eq!(ids.proposed, expected_proposed);
            }
            let entries = get_pool_entries(node0);
            assert!(entries.conflicted.is_empty());
            assert_eq!(
                entries.pending.keys().collect::<BTreeSet<_>>(),
                expected_pending.iter().collect()
            );
            assert_eq!(
                entries.proposed.keys().collect::<BTreeSet<_>>(),
                expected_proposed.iter().collect()
            );
            let chain = if proposed {
                &entries.proposed
            } else {
                &entries.pending
            };
            let (mut bytes, mut cycles) = (0, 0);
            for (index, hash) in txs_hash.iter().enumerate() {
                let entry = &chain[&H256::from(hash)];
                bytes += entry.size.value();
                cycles += entry.cycles.value();
                assert!(entry.size.value() > 0 && entry.cycles.value() > 0);
                assert_eq!(entry.ancestors_count.value(), index as u64 + 1);
                assert_eq!(entry.ancestors_size.value(), bytes);
                assert_eq!(entry.ancestors_cycles.value(), cycles);
            }
        }
    }
}
