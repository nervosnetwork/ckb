use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockReward, EpochExt, HeaderView},
    packed::{Byte32, Script},
};

pub trait ChainProvider: Sync + Send {
    type Store: ChainStore<'static>;

    fn store(&self) -> &Self::Store;

    fn script_config(&self) -> &ScriptConfig;

    fn genesis_hash(&self) -> Byte32;

    fn get_block_epoch(&self, hash: &Byte32) -> Option<EpochExt>;

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &HeaderView) -> Option<EpochExt>;

    fn finalize_block_reward(&self, parent: &HeaderView) -> Result<(Script, BlockReward), Error>;

    fn consensus(&self) -> &Consensus;
}
