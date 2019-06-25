use ckb_chain_spec::consensus::Consensus;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::transaction::OutPoint;
use ckb_core::Capacity;
use ckb_db::MemoryKeyValueDB;
use ckb_script::ScriptConfig;
use ckb_store::ChainKVStore;
use ckb_traits::ChainProvider;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::sync::Arc;

#[derive(Clone)]
pub struct DummyChainProvider {}

impl ChainProvider for DummyChainProvider {
    type Store = ChainKVStore<MemoryKeyValueDB>;

    fn store(&self) -> &Arc<ChainKVStore<MemoryKeyValueDB>> {
        unimplemented!();
    }

    fn script_config(&self) -> &ScriptConfig {
        unimplemented!();
    }

    fn genesis_hash(&self) -> &H256 {
        unimplemented!();
    }

    fn get_block_epoch(&self, _hash: &H256) -> Option<EpochExt> {
        unimplemented!();
    }

    fn next_epoch_ext(&self, _last_epoch: &EpochExt, _header: &Header) -> Option<EpochExt> {
        unimplemented!();
    }

    fn consensus(&self) -> &Consensus {
        unimplemented!();
    }

    fn finalize_block_reward(&self, _parent: &Header) -> Result<(Script, Capacity), FailureError> {
        unimplemented!();
    }
}

impl CellProvider for DummyChainProvider {
    fn cell(&self, _o: &OutPoint) -> CellStatus {
        unimplemented!();
    }
}
