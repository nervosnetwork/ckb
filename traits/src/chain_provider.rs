use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::Capacity;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::sync::Arc;

pub trait ChainProvider: Sync + Send {
    type Store: ChainStore;

    fn store(&self) -> &Arc<Self::Store>;

    fn script_config(&self) -> &ScriptConfig;

    fn genesis_hash(&self) -> &H256;

    fn get_block_epoch(&self, hash: &H256) -> Option<EpochExt>;

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt>;

    fn finalize_block_reward(&self, parent: &Header) -> Result<(Script, Capacity), FailureError>;

    fn consensus(&self) -> &Consensus;
}
