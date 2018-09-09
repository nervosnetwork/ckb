use super::super::block_verifier::CommitVerifier;
use super::super::error::{CommitError, Error};
use bigint::{H256, U256};
use chain::chain::{ChainBuilder, ChainProvider};
use chain::consensus::{Consensus, GenesisBuilder};
use chain::store::ChainKVStore;
use core::block::IndexedBlock;
use core::header::{Header, IndexedHeader, RawHeader, Seal};
use core::transaction::{
    CellInput, CellOutput, IndexedTransaction, OutPoint, ProposalShortId, Transaction, VERSION,
};
use core::uncle::UncleBlock;
use core::BlockNumber;
use db::memorydb::MemoryKeyValueDB;
use fnv::FnvHashMap;
use std::sync::Arc;

fn gen_block(
    parent_header: IndexedHeader,
    mut commit_transactions: Vec<IndexedTransaction>,
    proposal_transactions: Vec<ProposalShortId>,
) -> IndexedBlock {
    let now = 1 + parent_header.timestamp;
    let number = parent_header.number + 1;
    let nonce = parent_header.seal.nonce + 1;
    let difficulty = parent_header.difficulty + U256::from(1);
    let cellbase = create_cellbase(number);
    let header = Header {
        raw: RawHeader {
            number,
            difficulty,
            version: 0,
            parent_hash: parent_header.hash(),
            timestamp: now,
            txs_commit: H256::zero(),
            txs_proposal: H256::zero(),
            cellbase_id: cellbase.hash(),
            uncles_hash: H256::zero(),
        },
        seal: Seal {
            nonce,
            proof: Default::default(),
        },
    };
    commit_transactions.insert(0, cellbase);
    IndexedBlock {
        header: header.into(),
        uncles: vec![],
        commit_transactions,
        proposal_transactions,
    }
}

fn create_transaction(parent: H256) -> IndexedTransaction {
    let mut output = CellOutput::default();
    output.capacity = 100_000_000 / 100 as u64;
    let outputs: Vec<CellOutput> = vec![output.clone(); 100];

    Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::new(parent, 0), Default::default())],
        outputs,
    ).into()
}

fn create_cellbase(number: BlockNumber) -> IndexedTransaction {
    let inputs = vec![CellInput::new_cellbase_input(number)];
    let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
    Transaction::new(VERSION, Vec::new(), inputs, outputs).into()
}

fn push_uncle(block: &mut IndexedBlock, uncle: &IndexedBlock) {
    let uncle = UncleBlock {
        header: uncle.header.header.clone(),
        cellbase: uncle.commit_transactions.first().cloned().unwrap().into(),
        proposal_transactions: uncle.proposal_transactions.clone(),
    };

    block.uncles.push(uncle);
    block.header.uncles_hash = block.cal_uncles_hash();
    block.finalize_dirty();
}

#[test]
fn test_blank_proposal() {
    let tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(100_000_000, Vec::new(), H256::default()); 100],
    ).into();
    let mut root_hash = tx.hash();
    let genesis_builder = GenesisBuilder::default();
    let mut genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
    genesis_block.commit_transactions.push(tx);
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = FnvHashMap::default();
    let end = 21;

    let mut blocks: Vec<IndexedBlock> = Vec::new();
    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
    for i in 1..end {
        txs.insert(i, Vec::new());
        let tx = create_transaction(root_hash);
        root_hash = tx.hash();
        txs.get_mut(&i).unwrap().push(tx.clone());
        let new_block = gen_block(parent, vec![tx], vec![]);
        blocks.push(new_block.clone());
        parent = new_block.header;
    }

    for block in &blocks[0..10] {
        assert!(chain.process_block(&block, false).is_ok());
    }

    let verify = CommitVerifier::new(&blocks[10], Arc::clone(&chain)).verify();

    assert_eq!(verify, Err(Error::Commit(CommitError::Invalid)));
}

#[test]
fn test_uncle_proposal() {
    let tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(100_000_000, Vec::new(), H256::default()); 100],
    ).into();
    let mut root_hash = tx.hash();
    let genesis_builder = GenesisBuilder::default();
    let mut genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
    genesis_block.commit_transactions.push(tx);
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let uncle = gen_block(parent.clone(), vec![], proposal_ids);
    let mut block = gen_block(parent.clone(), vec![], vec![]);

    push_uncle(&mut block, &uncle);

    assert!(chain.process_block(&block, false).is_ok());

    parent = block.header;

    let new_block = gen_block(parent, txs, vec![]);

    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));
}

#[test]
fn test_block_proposal() {
    let tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(100_000_000, Vec::new(), H256::default()); 100],
    ).into();
    let mut root_hash = tx.hash();
    let genesis_builder = GenesisBuilder::default();
    let mut genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
    genesis_block.commit_transactions.push(tx);
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids);

    assert!(chain.process_block(&block, false).is_ok());

    parent = block.header;

    let new_block = gen_block(parent, txs, vec![]);

    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));
}

#[test]
fn test_proposal_timeout() {
    let tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(100_000_000, Vec::new(), H256::default()); 100],
    ).into();
    let mut root_hash = tx.hash();
    let genesis_builder = GenesisBuilder::default();
    let mut genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
    genesis_block.commit_transactions.push(tx);
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids);
    assert!(chain.process_block(&block, false).is_ok());
    parent = block.header;

    let timeout = chain.consensus().transaction_propagation_timeout;

    for _ in 0..timeout - 1 {
        let block = gen_block(parent, vec![], vec![]);
        assert!(chain.process_block(&block, false).is_ok());
        parent = block.header;
    }

    let new_block = gen_block(parent.clone(), txs.clone(), vec![]);
    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));

    let block = gen_block(parent, vec![], vec![]);
    assert!(chain.process_block(&block, false).is_ok());
    parent = block.header;

    let new_block = gen_block(parent.clone(), txs, vec![]);
    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Err(Error::Commit(CommitError::Invalid)));
}
