use crate::chain_state::ChainState;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::transaction::ProposalShortId;
use ckb_core::{block::BlockBuilder, header::HeaderBuilder};
use ckb_db::{KeyValueDB, MemoryKeyValueDB};
use ckb_store::COLUMNS;
use ckb_store::{ChainKVStore, ChainStore, StoreBatch};
use ckb_util::FnvHashSet;
use std::collections::BTreeMap;
use std::sync::Arc;

fn insert_block_proposals<T>(store: &ChainKVStore<T>, proposals: Vec<ProposalShortId>)
where
    T: KeyValueDB,
{
    let mut blocks = Vec::with_capacity(proposals.len());
    let tip_header = store.get_tip_header().expect("tip");
    let mut parent_hash = tip_header.hash().to_owned();
    let mut parent_number = tip_header.number();
    for proposal in proposals {
        let header = HeaderBuilder::default()
            .parent_hash(parent_hash.clone())
            .number(parent_number + 1)
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
    let mut batch = store.new_batch().unwrap();
    for b in blocks {
        batch.insert_block(&b).unwrap();
        batch.attach_block(&b).unwrap();
    }
    batch.commit().unwrap();
}

fn new_memory_store() -> ChainKVStore<MemoryKeyValueDB> {
    ChainKVStore::new(MemoryKeyValueDB::open(COLUMNS as usize))
}

#[test]
fn proposal_table_init() {
    let store = Arc::new(new_memory_store());
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
        let id = ProposalShortId::from_slice(&[i; 10]).unwrap();
        proposal_ids.push(id);
        let mut set = FnvHashSet::default();
        set.insert(id);
        proposal_table.insert(u64::from(i + 1), set);
    }

    insert_block_proposals(&store, proposal_ids);

    let inited_proposal_table = ChainState::init_proposal_ids(store.as_ref(), proposal_window, 5);
    assert_eq!(inited_proposal_table.all(), &proposal_table)
}
