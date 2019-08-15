use ckb_chain_spec::consensus::Consensus;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockReward, EpochExt, HeaderView},
    packed::Script,
    H256,
};
use failure::Error as FailureError;

pub trait ChainProvider: Sync + Send {
    type Store: ChainStore<'static>;

    fn store(&self) -> &Self::Store;

    fn script_config(&self) -> &ScriptConfig;

    fn genesis_hash(&self) -> &H256;

    fn get_block_epoch(&self, hash: &H256) -> Option<EpochExt>;

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &HeaderView) -> Option<EpochExt>;

    fn finalize_block_reward(
        &self,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), FailureError>;

    fn consensus(&self) -> &Consensus;
}
