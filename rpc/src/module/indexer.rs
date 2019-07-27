use ckb_indexer::IndexerStore;
use ckb_jsonrpc_types::{BlockNumber, CellTransaction, LiveCell, LockHashIndexState, Unsigned};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use numext_fixed_hash::H256;

#[rpc]
pub trait IndexerRpc {
    #[rpc(name = "get_live_cells_by_lock_hash")]
    fn get_live_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _page: Unsigned,
        _per_page: Unsigned,
        _reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>>;

    #[rpc(name = "get_transactions_by_lock_hash")]
    fn get_transactions_by_lock_hash(
        &self,
        _lock_hash: H256,
        _page: Unsigned,
        _per_page: Unsigned,
        _reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>>;

    #[rpc(name = "index_lock_hash")]
    fn index_lock_hash(
        &self,
        _lock_hash: H256,
        _index_from: Option<BlockNumber>,
    ) -> Result<LockHashIndexState>;

    #[rpc(name = "deindex_lock_hash")]
    fn deindex_lock_hash(&self, _lock_hash: H256) -> Result<()>;

    #[rpc(name = "get_lock_hash_index_states")]
    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>>;
}

pub(crate) struct IndexerRpcImpl<WS> {
    pub store: WS,
}

impl<WS: IndexerStore + 'static> IndexerRpc for IndexerRpcImpl<WS> {
    fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Unsigned,
        per_page: Unsigned,
        reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>> {
        let per_page = (per_page.0 as usize).min(50);
        Ok(self
            .store
            .get_live_cells(
                &lock_hash,
                (page.0 as usize).saturating_mul(per_page),
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
        page: Unsigned,
        per_page: Unsigned,
        reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>> {
        let per_page = (per_page.0 as usize).min(50);
        Ok(self
            .store
            .get_transactions(
                &lock_hash,
                (page.0 as usize).saturating_mul(per_page),
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
            .insert_lock_hash(&lock_hash, index_from.map(|number| number.0));
        Ok(LockHashIndexState {
            lock_hash,
            block_number: BlockNumber(state.block_number),
            block_hash: state.block_hash,
        })
    }

    fn deindex_lock_hash(&self, lock_hash: H256) -> Result<()> {
        self.store.remove_lock_hash(&lock_hash);
        Ok(())
    }

    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>> {
        let states = self
            .store
            .get_lock_hash_index_states()
            .into_iter()
            .map(|(lock_hash, state)| LockHashIndexState {
                lock_hash,
                block_number: BlockNumber(state.block_number),
                block_hash: state.block_hash,
            })
            .collect();
        Ok(states)
    }
}
