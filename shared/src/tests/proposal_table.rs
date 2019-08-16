use crate::chain_state::ChainState;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_db::RocksDB;
use ckb_store::{ChainDB, ChainStore, COLUMNS};
use ckb_types::{
    core::{BlockBuilder, HeaderBuilder},
    packed::ProposalShortId,
    prelude::*,
};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;

fn insert_block_proposals(store: &ChainDB, proposals: Vec<ProposalShortId>) {
    let mut blocks = Vec::with_capacity(proposals.len());
    let tip_header = store.get_tip_header().expect("tip");
    let mut parent_hash = tip_header.hash().to_owned();
    let mut parent_number = tip_header.number();
    for proposal in proposals {
        let header = HeaderBuilder::default()
            .parent_hash(parent_hash.clone())
            .number((parent_number + 1).pack())
            .build();
        parent_hash = header.hash().to_owned();
        parent_number += 1;
        blocks.push(
            BlockBuilder::default()
                .header(header)
                .proposal(proposal)
                .build(),
        );
    }
    let txn = store.begin_transaction();
    for b in blocks {
        txn.insert_block(&b).unwrap();
        txn.attach_block(&b).unwrap();
    }
    txn.commit().unwrap();
}

fn new_store() -> ChainDB {
    ChainDB::new(RocksDB::open_tmp(COLUMNS))
}

#[test]
fn proposal_table_init() {
    let store = Arc::new(new_store());
    let mut consensus = Consensus::default();
    let proposal_window = ProposalWindow(3, 5);
    consensus.tx_proposal_window = proposal_window;

    ChainState::init(
        &store,
        Arc::new(consensus),
        Default::default(),
        Default::default(),
    )
    .unwrap();

    let mut proposal_ids = Vec::new();
    let mut proposal_table = BTreeMap::new();

    for i in 0..5 {
        let id: ProposalShortId = [i; 10].pack();
        proposal_ids.push(id.clone());
        let mut set = HashSet::default();
        set.insert(id);
        proposal_table.insert(u64::from(i + 1), set);
    }

    insert_block_proposals(&store, proposal_ids);

    let inited_proposal_table = ChainState::init_proposal_ids(store.as_ref(), proposal_window, 5);
    assert_eq!(inited_proposal_table.all(), &proposal_table)
}
