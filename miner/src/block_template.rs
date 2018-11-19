use bigint::H256;
use chain::chain::ChainProvider;
use chain::error::Error;
use core::header::{Header, RawHeader};
use core::transaction::{
    CellInput, CellOutput, IndexedTransaction, ProposalShortId, Transaction, VERSION,
};
use core::uncle::{uncles_hash, UncleBlock};
use pool::TransactionPool;
use std::cmp;
use std::sync::Arc;
use time::now_ms;

#[derive(Serialize, Debug)]
pub struct BlockTemplate {
    pub raw_header: RawHeader,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<IndexedTransaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

pub fn build_block_template<C: ChainProvider + 'static>(
    chain: &Arc<C>,
    tx_pool: &Arc<TransactionPool<C>>,
    redeem_script_hash: H256,
    max_tx: usize,
    max_prop: usize,
) -> Result<BlockTemplate, Error> {
    let header = chain.tip_header().read().header.clone();
    let now = cmp::max(now_ms(), header.timestamp + 1);
    let difficulty = chain.calculate_difficulty(&header).expect("get difficulty");

    let proposal_transactions = tx_pool.prepare_proposal(max_prop);
    let mut commit_transactions = tx_pool.get_mineable_transactions(max_tx);
    let cellbase =
        create_cellbase_transaction(&chain, &header, &commit_transactions, redeem_script_hash)?;
    let uncles = chain.get_tip_uncles();
    let cellbase_id = cellbase.hash();

    commit_transactions.insert(0, cellbase);

    let raw_header = RawHeader::new(
        &header,
        commit_transactions.iter(),
        proposal_transactions.iter(),
        now,
        difficulty,
        cellbase_id,
        uncles_hash(&uncles),
    );

    let block = BlockTemplate {
        raw_header,
        uncles,
        commit_transactions,
        proposal_transactions,
    };

    Ok(block)
}

fn create_cellbase_transaction<C: ChainProvider + 'static>(
    chain: &Arc<C>,
    header: &Header,
    transactions: &[IndexedTransaction],
    redeem_script_hash: H256,
) -> Result<IndexedTransaction, Error> {
    // NOTE: To generate different cellbase txid, we put header number in the input script
    let inputs = vec![CellInput::new_cellbase_input(header.raw.number + 1)];
    // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
    // but the truth is we will modify this after we designed lock script anyway, so let's
    // stick to the simpler way and just convert everything to a single string, then to UTF8
    // bytes, they really serve the same purpose at the moment
    let block_reward = chain.block_reward(header.raw.number + 1);
    let mut fee = 0;
    for transaction in transactions {
        fee += chain.calculate_transaction_fee(transaction)?;
    }

    let outputs = vec![CellOutput::new(
        block_reward + fee,
        Vec::new(),
        redeem_script_hash,
    )];

    Ok(Transaction::new(VERSION, Vec::new(), inputs, outputs).into())
}

#[cfg(test)]
pub mod test {
    use super::*;
    use bigint::H256;
    use chain::chain::ChainBuilder;
    use chain::store::ChainKVStore;
    use chain::DummyPowEngine;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_notify::Notify;
    use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};
    use core::block::IndexedBlock;
    use pool::PoolConfig;

    #[test]
    fn test_block_template() {
        let chain = Arc::new(
            ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
                .build()
                .unwrap(),
        );

        let pow_engine = Arc::new(DummyPowEngine::new());

        let tx_pool = Arc::new(TransactionPool::new(
            PoolConfig::default(),
            Arc::clone(&chain),
            Notify::default(),
        ));

        let block_template =
            build_block_template(&chain, &tx_pool, H256::from(0), 1000, 1000).unwrap();

        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        //do not verfiy pow here
        let header = raw_header.with_seal(Default::default());

        let block = IndexedBlock {
            header: header.into(),
            uncles,
            commit_transactions,
            proposal_transactions: proposal_transactions,
        };

        let resolver = HeaderResolverWrapper::new(&block.header, &chain);
        let header_verify = HeaderVerifier::new(resolver, &pow_engine);

        assert!(header_verify.verify().is_ok());

        let block_verfiy = BlockVerifier::new(&block, &chain, &pow_engine);
        assert!(block_verfiy.verify().is_ok());
    }
}
