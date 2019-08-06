use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::reward::BlockReward;
use ckb_core::script::Script;
use ckb_error::Error;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use numext_fixed_hash::H256;

pub trait ChainProvider: Sync + Send {
    type Store: ChainStore<'static>;

    fn store(&self) -> &Self::Store;

    fn script_config(&self) -> &ScriptConfig;

    fn genesis_hash(&self) -> &H256;

    fn get_block_epoch(&self, hash: &H256) -> Option<EpochExt>;

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt>;

    fn finalize_block_reward(&self, parent: &Header) -> Result<(Script, BlockReward), Error>;

    fn consensus(&self) -> &Consensus;
}
