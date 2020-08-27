use ckb_indexer::IndexerStore;
use ckb_jsonrpc_types::{
    BlockNumber, CellTransaction, LiveCell, LockHashCapacity, LockHashIndexState, Uint64,
};
use ckb_types::{prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

#[rpc(server)]
pub trait IndexerRpc {
    #[rpc(name = "deprecated.get_live_cells_by_lock_hash")]
    fn get_live_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _page: Uint64,
        _per_page: Uint64,
        _reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>>;

    #[rpc(name = "deprecated.get_transactions_by_lock_hash")]
    fn get_transactions_by_lock_hash(
        &self,
        _lock_hash: H256,
        _page: Uint64,
        _per_page: Uint64,
        _reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>>;

    #[rpc(name = "deprecated.index_lock_hash")]
    fn index_lock_hash(
        &self,
        _lock_hash: H256,
        _index_from: Option<BlockNumber>,
    ) -> Result<LockHashIndexState>;

    #[rpc(name = "deprecated.deindex_lock_hash")]
    fn deindex_lock_hash(&self, _lock_hash: H256) -> Result<()>;

    #[rpc(name = "deprecated.get_lock_hash_index_states")]
    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>>;

    #[rpc(name = "deprecated.get_capacity_by_lock_hash")]
    fn get_capacity_by_lock_hash(&self, _lock_hash: H256) -> Result<Option<LockHashCapacity>>;
}

pub(crate) struct IndexerRpcImpl<WS> {
    pub store: WS,
}

impl<WS: IndexerStore + 'static> IndexerRpc for IndexerRpcImpl<WS> {
    fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>> {
        let lock_hash = lock_hash.pack();
        let per_page = (per_page.value() as usize).min(50);
        Ok(self
            .store
            .get_live_cells(
                &lock_hash,
                (page.value() as usize).saturating_mul(per_page),
                per_page,
                reverse_order.unwrap_or_default(),
            )
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn get_transactions_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>> {
        let lock_hash = lock_hash.pack();
        let per_page = (per_page.value() as usize).min(50);
        Ok(self
            .store
            .get_transactions(
                &lock_hash,
                (page.value() as usize).saturating_mul(per_page),
                per_page,
                reverse_order.unwrap_or_default(),
            )
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn index_lock_hash(
        &self,
        lock_hash: H256,
        index_from: Option<BlockNumber>,
    ) -> Result<LockHashIndexState> {
        let state = self
            .store
            .insert_lock_hash(&lock_hash.pack(), index_from.map(Into::into));
        Ok(LockHashIndexState {
            lock_hash,
            block_number: state.block_number.into(),
            block_hash: state.block_hash.unpack(),
        })
    }

    fn deindex_lock_hash(&self, lock_hash: H256) -> Result<()> {
        self.store.remove_lock_hash(&lock_hash.pack());
        Ok(())
    }

    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>> {
        let states = self
            .store
            .get_lock_hash_index_states()
            .into_iter()
            .map(|(lock_hash, state)| LockHashIndexState {
                lock_hash: lock_hash.unpack(),
                block_number: state.block_number.into(),
                block_hash: state.block_hash.unpack(),
            })
            .collect();
        Ok(states)
    }

    fn get_capacity_by_lock_hash(&self, lock_hash: H256) -> Result<Option<LockHashCapacity>> {
        let lock_hash = lock_hash.pack();
        Ok(self.store.get_capacity(&lock_hash).map(Into::into))
    }
}
