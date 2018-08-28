use bigint::H256;
use chain::chain::{ChainProvider, Error};
use core::header::{Header, RawHeader};
use core::transaction::{CellInput, CellOutput, Transaction, VERSION};
use core::uncle::{uncles_hash, UncleBlock};
use pool::TransactionPool;
use std::cmp;
use std::sync::Arc;
use time::now_ms;

#[derive(Serialize, Debug)]
pub struct BlockTemplate {
    pub raw_header: RawHeader,
    pub transactions: Vec<Transaction>,
    pub uncles: Vec<UncleBlock>,
}

pub fn build_block_template<C: ChainProvider + 'static>(
    chain: &Arc<C>,
    tx_pool: &Arc<TransactionPool<C>>,
) -> Result<BlockTemplate, Error> {
    let header = chain.tip_header().read().header.clone();
    let now = cmp::max(now_ms(), header.timestamp + 1);
    let difficulty = chain.calculate_difficulty(&header).expect("get difficulty");

    let mut transactions = tx_pool.prepare_mineable_transactions();
    let cellbase = create_cellbase_transaction(&chain, &header, &transactions)?;
    let uncles = chain.get_tip_uncles();

    let raw_header = RawHeader::new(
        &header,
        transactions.iter(),
        now,
        difficulty,
        cellbase.hash(),
        uncles_hash(&uncles),
    );

    transactions.insert(0, cellbase);

    let block = BlockTemplate {
        transactions,
        uncles,
        raw_header,
    };

    Ok(block)
}

fn create_cellbase_transaction<C: ChainProvider + 'static>(
    chain: &Arc<C>,
    header: &Header,
    transactions: &[Transaction],
) -> Result<Transaction, Error> {
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
        // self.config.redeem_script_hash,
        H256::default(),
    )];

    Ok(Transaction::new(VERSION, Vec::new(), inputs, outputs))
}
