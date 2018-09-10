use bigint::H256;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    // Max number of transactions this miner will assemble in a block
    pub max_tx: usize,
    pub new_transactions_threshold: u16,
    pub ethash_path: Option<String>,
    pub redeem_script_hash: H256,
}
