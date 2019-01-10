//! Top-level Pool type, methods, and tests
use super::trace::{Trace, TraceMap};
use super::types::{
    InsertionResult, Orphan, PendingQueue, Pool, PoolConfig, PoolError, ProposedQueue, TxStage,
    TxoStatus,
};
use channel::{self, select, Receiver, Sender};
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_notify::{ForkBlocks, MsgNewTip, MsgSwitchFork, NotifyController};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_verification::{TransactionError, TransactionVerifier};
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::thread::{self, JoinHandle};

#[cfg(test)]
use ckb_core::BlockNumber;

const TXS_POOL_SUBSCRIBER: &str = "txs_pool";

pub type TxsArgs = (usize, usize);
pub type TxsReturn = (Vec<ProposalShortId>, Vec<Transaction>);

#[derive(Clone)]
pub struct TransactionPoolController {
    get_proposal_commit_transactions_sender: Sender<Request<TxsArgs, TxsReturn>>,
    get_potential_transactions_sender: Sender<Request<(), Vec<Transaction>>>,
    contains_key_sender: Sender<Request<ProposalShortId, bool>>,
    get_transaction_sender: Sender<Request<ProposalShortId, Option<Transaction>>>,
    add_transaction_sender: Sender<Request<Transaction, Result<InsertionResult, PoolError>>>,
}

pub struct TransactionPoolReceivers {
    get_proposal_commit_transactions_receiver: Receiver<Request<TxsArgs, TxsReturn>>,
    get_potential_transactions_receiver: Receiver<Request<(), Vec<Transaction>>>,
    contains_key_receiver: Receiver<Request<ProposalShortId, bool>>,
    get_transaction_receiver: Receiver<Request<ProposalShortId, Option<Transaction>>>,
    add_transaction_receiver: Receiver<Request<Transaction, Result<InsertionResult, PoolError>>>,
}

impl TransactionPoolController {
    pub fn build() -> (TransactionPoolController, TransactionPoolReceivers) {
        let (get_proposal_commit_transactions_sender, get_proposal_commit_transactions_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (get_potential_transactions_sender, get_potential_transactions_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (contains_key_sender, contains_key_receiver) = channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (get_transaction_sender, get_transaction_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (add_transaction_sender, add_transaction_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        (
            TransactionPoolController {
                get_proposal_commit_transactions_sender,
                get_potential_transactions_sender,
                contains_key_sender,
                get_transaction_sender,
                add_transaction_sender,
            },
            TransactionPoolReceivers {
                get_proposal_commit_transactions_receiver,
                get_potential_transactions_receiver,
                contains_key_receiver,
                get_transaction_receiver,
                add_transaction_receiver,
            },
        )
    }

    pub fn get_proposal_commit_transactions(
        &self,
        max_prop: usize,
        max_tx: usize,
    ) -> (Vec<ProposalShortId>, Vec<Transaction>) {
        Request::call(
            &self.get_proposal_commit_transactions_sender,
            (max_prop, max_tx),
        )
        .expect("get_proposal_commit_transactions() failed")
    }

    pub fn get_potential_transactions(&self) -> Vec<Transaction> {
        Request::call(&self.get_potential_transactions_sender, ())
            .expect("get_potential_transactions() failed")
    }

    pub fn contains_key(&self, id: ProposalShortId) -> bool {
        Request::call(&self.contains_key_sender, id).expect("contains_key() failed")
    }

    pub fn get_transaction(&self, id: ProposalShortId) -> Option<Transaction> {
        Request::call(&self.get_transaction_sender, id).expect("get_transaction() failed")
    }

    pub fn add_transaction(&self, tx: Transaction) -> Result<InsertionResult, PoolError> {
        Request::call(&self.add_transaction_sender, tx).expect("add_transaction() failed")
    }
}

/// The pool itself.
pub struct TransactionPoolService<CI> {
    config: PoolConfig,
    /// The short id that has not been proposed
    pending: PendingQueue,
    /// The short id that has been proposed
    proposed: ProposedQueue,
    /// The  pool
    pool: Pool,
    /// Orphans in the pool
    orphan: Orphan,
    /// cache for conflict transaction
    cache: LruCache<ProposalShortId, Transaction>,

    shared: Shared<CI>,
    notify: NotifyController,

    trace: TraceMap,
}

impl<CI> CellProvider for TransactionPoolService<CI>
where
    CI: ChainIndex,
{
    fn cell(&self, o: &OutPoint) -> CellStatus {
        match { self.pool.txo_status(o) } {
            TxoStatus::Spent => CellStatus::Dead,
            TxoStatus::InPool => CellStatus::Live(self.pool.get_output(o).unwrap()),
            TxoStatus::Unknown => self.shared.cell(o),
        }
    }

    fn cell_at(&self, _o: &OutPoint, _parent: &H256) -> CellStatus {
        unreachable!()
    }
}

impl<CI> TransactionPoolService<CI>
where
    CI: ChainIndex + 'static,
{
    /// Create a new transaction pool
    pub fn new(
        config: PoolConfig,
        shared: Shared<CI>,
        notify: NotifyController,
    ) -> TransactionPoolService<CI> {
        let n = shared.tip_header().read().number();
        let cache_size = config.max_cache_size;
        let prop_cap = ProposedQueue::cap();
        let ids = shared.union_proposal_ids_n(n, prop_cap);
        let trace_size = config.trace.unwrap_or(0);

        TransactionPoolService {
            config,
            pending: PendingQueue::new(),
            proposed: ProposedQueue::new(n, ids),
            pool: Pool::new(),
            orphan: Orphan::new(),
            cache: LruCache::new(cache_size),
            shared,
            notify,
            trace: TraceMap::new(trace_size),
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        receivers: TransactionPoolReceivers,
    ) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_tip_receiver = self.notify.subscribe_new_tip(TXS_POOL_SUBSCRIBER);
        let switch_fork_receiver = self.notify.subscribe_switch_fork(TXS_POOL_SUBSCRIBER);
        thread_builder
            .spawn(move || loop {
                select!{
                    recv(new_tip_receiver) -> msg => self.handle_new_tip(msg),
                    recv(switch_fork_receiver) -> msg => self.handle_switch_fork(msg),

                    recv(receivers.get_proposal_commit_transactions_receiver) -> msg => {
                        self.handle_get_proposal_commit_transactions(msg)
                    },
                    recv(receivers.get_potential_transactions_receiver) -> msg => match msg {
                        Ok(Request { responder, ..}) => {
                            let _ = responder.send(self.get_potential_transactions());
                        }
                        _ => {
                            error!(target: "txs_pool", "channel get_potential_transactions_receiver closed");
                        }
                    },
                    recv(receivers.contains_key_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: id }) => {
                            let _ = responder.send(self.contains_key(&id));
                        }
                        _ => {
                            error!(target: "txs_pool", "channel contains_key_receiver closed");
                        }
                    },
                    recv(receivers.get_transaction_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: id }) => {
                            let _ = responder.send(self.get(&id));
                        }
                        _ => {
                            error!(target: "txs_pool", "channel get_transaction_receiver closed");
                        }
                    },
                    recv(receivers.add_transaction_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: tx }) => {
                            let _ = responder.send(self.add_transaction(tx));
                        }
                        _ => {
                            error!(target: "txs_pool", "channel add_transaction_receiver closed");
                        }
                    }
                }
            }).expect("Start TransactionPoolService failed!")
    }

    fn handle_new_tip(&mut self, msg: Result<MsgNewTip, channel::RecvError>) {
        match msg {
            Ok(block) => self.reconcile_block(&block),
            _ => {
                error!(target: "txs_pool", "channel new_tip_receiver closed");
            }
        }
    }

    fn handle_switch_fork(&mut self, msg: Result<MsgSwitchFork, channel::RecvError>) {
        match msg {
            Ok(blocks) => self.switch_fork(&blocks),
            _ => {
                error!(target: "txs_pool", "channel switch_fork_receiver closed");
            }
        }
    }

    fn handle_get_proposal_commit_transactions(
        &self,
        msg: Result<Request<TxsArgs, TxsReturn>, channel::RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (max_prop, max_tx),
            }) => {
                let proposal_transactions = self.prepare_proposal(max_prop);
                let commit_transactions = self.get_mineable_transactions(max_tx);
                let _ = responder.send((proposal_transactions, commit_transactions));
            }
            _ => {
                error!(target: "txs_pool", "channel get_proposal_commit_transactions_receiver closed");
            }
        }
    }

    pub(crate) fn switch_fork(&mut self, blks: &ForkBlocks) {
        for b in blks.old_blks() {
            let bn = b.header().number();
            let mut txs = b.commit_transactions().to_vec();
            txs.reverse();

            //remove proposed id, txs can be already in pool
            if let Some(rm_txs) = self.proposed.remove(bn) {
                for (id, x) in rm_txs {
                    if let Some(tx) = x {
                        self.pending.insert(id, tx);
                    } else if let Some(txs) = self.pool.remove(&id) {
                        self.pending.insert(id, txs[0].clone());

                        for tx in txs.iter().skip(1) {
                            self.cache.insert(tx.proposal_short_id(), tx.clone());
                        }
                    } else if let Some(tx) = self.cache.remove(&id) {
                        self.pending.insert(id, tx);
                    } else if let Some(tx) = self.orphan.remove(&id) {
                        self.pending.insert(id, tx);
                    }
                }
            }

            //readd txs in proposed queue
            if let Some(frt_ids) = self.proposed.front().cloned() {
                for id in frt_ids {
                    if let Some(txs) = self.pool.remove(&id) {
                        self.proposed.insert_without_check(id, txs[0].clone());
                        for tx in txs.iter().skip(1) {
                            self.cache.insert(tx.proposal_short_id(), tx.clone());
                        }
                    } else if let Some(tx) = self.cache.remove(&id) {
                        self.proposed.insert_without_check(id, tx);
                    } else if let Some(tx) = self.orphan.remove(&id) {
                        self.proposed.insert_without_check(id, tx);
                    }
                }
            }

            //readd txs
            for tx in txs {
                if tx.is_cellbase() {
                    continue;
                }
                self.pool.add_transaction(tx.clone());
            }
        }

        // We may not need readd timeout transactions in pool, because new main chain is mostly longer
        for blk in blks.new_blks() {
            self.reconcile_block(&blk);
        }
    }

    fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.cache.contains_key(id)
            || self.pool.contains_key(id)
            || self.orphan.contains_key(id)
            || self.proposed.contains_key(id)
    }

    fn get(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get(id)
            .cloned()
            .or_else(|| self.proposed.get(id).cloned())
            .or_else(|| self.pool.get(id).cloned())
            .or_else(|| self.orphan.get(id).cloned())
            .or_else(|| self.cache.get(id).cloned())
    }

    /// Get the size of transactions in the pool
    pub(crate) fn pool_size(&self) -> usize {
        self.pool.size()
    }

    /// Get the size of orphans in the pool
    pub(crate) fn orphan_size(&self) -> usize {
        self.orphan.size()
    }

    /// Get the size of pending
    /// NOTE: may remove this method later
    #[cfg(test)]
    pub(crate) fn pending_size(&self) -> usize {
        self.pending.size()
    }

    /// Get the size of proposed
    /// NOTE: may remove this method later
    #[cfg(test)]
    pub(crate) fn proposed_size(&self) -> usize {
        self.proposed.size()
    }

    /// Get the size of cache
    /// NOTE: may remove this method later
    #[cfg(test)]
    pub(crate) fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Get the total size (transactions + orphans) of the pool
    pub(crate) fn total_size(&self) -> usize {
        self.pool_size() + self.orphan_size()
    }

    pub(crate) fn add_transaction(
        &mut self,
        tx: Transaction,
    ) -> Result<InsertionResult, PoolError> {
        match { self.proposed.insert(tx) } {
            TxStage::Mineable(x) => self.add_to_pool(x),
            TxStage::Unknown(x) => {
                self.pending.insert(x.proposal_short_id(), x);
                Ok(InsertionResult::Unknown)
            }
            _ => Ok(InsertionResult::Proposed),
        }
    }

    pub(crate) fn trace_transaction(
        &mut self,
        tx: Transaction,
    ) -> Result<InsertionResult, PoolError> {
        let tx_hash = tx.hash();
        match { self.proposed.insert(tx) } {
            TxStage::Mineable(x) => self.add_to_pool(x),
            TxStage::Unknown(x) => {
                if self.config.trace_enable() {
                    self.trace
                        .add_pending(&tx_hash, "unknown tx, add to pending");
                }
                self.pending.insert(x.proposal_short_id(), x);
                Ok(InsertionResult::Unknown)
            }
            _ => Ok(InsertionResult::Proposed),
        }
    }

    pub(crate) fn get_transaction_traces(&self, hash: &H256) -> Option<&Vec<Trace>> {
        self.trace.get(hash)
    }

    pub(crate) fn prepare_proposal(&self, n: usize) -> Vec<ProposalShortId> {
        self.pending.fetch(n)
    }

    /// NOTE: may remove this method later
    #[cfg(test)]
    pub(crate) fn propose_transaction(&mut self, bn: BlockNumber, tx: Transaction) {
        match self.proposed.insert_with_n(bn, tx) {
            TxStage::Mineable(x) => {
                let _ = self.add_to_pool(x);
            }
            TxStage::TimeOut(x) | TxStage::Fork(x) => {
                self.pending.insert(x.proposal_short_id(), x);
            }
            _ => {}
        };
    }

    pub(crate) fn get_mineable_transactions(&self, max: usize) -> Vec<Transaction> {
        self.pool.get_mineable_transactions(max)
    }

    // Get all transactions that can be in next block, cache should added
    fn get_potential_transactions(&self) -> Vec<Transaction> {
        self.pool.get_mineable_transactions(self.pool.size())
    }

    /// Attempts to add a transaction to the memory pool.
    pub(crate) fn add_to_pool(&mut self, tx: Transaction) -> Result<InsertionResult, PoolError> {
        // Do we have the capacity to accept this transaction?
        self.is_acceptable()?;

        if tx.is_cellbase() {
            return Err(PoolError::Cellbase);
        }

        self.check_duplicate(&tx)?;

        let inputs = tx.input_pts();
        let deps = tx.dep_pts();

        let mut unknowns = Vec::new();

        {
            let rtx = self.resolve_transaction(&tx);

            for (i, cs) in rtx.input_cells.iter().enumerate() {
                match cs {
                    CellStatus::Unknown => {
                        unknowns.push(inputs[i].clone());
                    }
                    CellStatus::Dead => {
                        self.cache.insert(tx.proposal_short_id(), tx);
                        return Err(PoolError::DoubleSpent);
                    }
                    _ => {}
                }
            }

            for (i, cs) in rtx.dep_cells.iter().enumerate() {
                match cs {
                    CellStatus::Unknown => {
                        unknowns.push(deps[i].clone());
                    }
                    CellStatus::Dead => {
                        self.cache.insert(tx.proposal_short_id(), tx);
                        return Err(PoolError::DoubleSpent);
                    }
                    _ => {}
                }
            }

            if unknowns.is_empty() {
                // TODO: Parallel
                TransactionVerifier::new(&rtx)
                    .verify(self.shared.consensus().max_block_cycles())
                    .map_err(PoolError::InvalidTx)?;
            }
        }

        if !unknowns.is_empty() {
            if self.config.trace_enable() {
                self.trace
                    .add_orphan(&tx.hash(), format!("unknowns {:?}", unknowns));
            }
            self.orphan.add_transaction(tx, unknowns.into_iter());
            return Ok(InsertionResult::Orphan);
        } else {
            if self.config.trace_enable() {
                self.trace
                    .add_commit(&tx.hash(), format!("add to commit pool"));
            }
            self.pool.add_transaction(tx.clone());
            self.reconcile_orphan(&tx);
        }

        Ok(InsertionResult::Normal)
    }

    /// Updates the pool and orphan pool with new transactions.
    pub(crate) fn reconcile_orphan(&mut self, tx: &Transaction) {
        let txs = self.orphan.reconcile_transaction(tx);

        for tx in txs {
            let rtx = self.resolve_transaction(&tx);
            let rs =
                TransactionVerifier::new(&rtx).verify(self.shared.consensus().max_block_cycles());
            if self.config.trace_enable() {
                self.trace.add_commit(
                    &tx.hash(),
                    format!(
                        "removed from orphan, prepare add to commit, verify result {:?}",
                        rs
                    ),
                );
            }
            if rs.is_ok() {
                self.pool.add_transaction(tx);
            } else if rs == Err(TransactionError::DoubleSpent) {
                self.cache.insert(tx.proposal_short_id(), tx);
            }
        }
    }

    /// Updates the pool with the details of a new block.
    // TODO: call it in order
    pub(crate) fn reconcile_block(&mut self, b: &Block) {
        let txs = b.commit_transactions();
        let bn = b.header().number();
        let ids = b.union_proposal_ids();

        // must do this first
        {
            for tx in txs {
                if tx.is_cellbase() {
                    continue;
                }

                self.reconcile_orphan(tx);
            }
        }

        // must do this secondly
        {
            for tx in txs {
                if tx.is_cellbase() {
                    continue;
                }
                if self.config.trace_enable() {
                    self.trace.committed(
                        &tx.hash(),
                        format!(
                            "committed in block number({:?})-hash({:#x})",
                            b.header().number(),
                            b.header().hash()
                        ),
                    );
                }
                self.pool.commit_transaction(tx);
            }
        }

        {
            if let Some(time_out_ids) = self.proposed.mineable_front() {
                for id in time_out_ids {
                    if let Some(txs) = self.pool.remove(id) {
                        for tx in txs {
                            if self.config.trace_enable() {
                                self.trace.timeout(
                                    &tx.hash(),
                                    "tx proposal timeout, removed from pool, readd to pending",
                                );
                            }
                            self.pending.insert(tx.proposal_short_id(), tx);
                        }
                    } else if let Some(tx) = self.orphan.remove(id) {
                        if self.config.trace_enable() {
                            self.trace.timeout(
                                &tx.hash(),
                                "tx proposal timeout, removed from orphan, readd to pending",
                            );
                        }
                        self.pending.insert(tx.proposal_short_id(), tx);
                    }
                }
            }
        }

        let new_txs = {
            for id in &ids {
                if let Some(tx) = self.pending.remove(id).or_else(|| self.cache.remove(id)) {
                    if self.config.trace_enable() {
                        self.trace.proposed(
                            &tx.hash(),
                            format!(
                                "{:?} proposed in block number({:?})-hash({:#x})",
                                id,
                                b.header().number(),
                                b.header().hash()
                            ),
                        );
                    }
                    self.proposed.insert_without_check(id.clone(), tx);
                }
            }

            self.proposed.reconcile(bn, ids).unwrap_or_else(|error| {
                error!(target: "txs_pool", "Failed to proposed reconcile {:?}", error);
                vec![]
            })
        };

        // We can sort it by some rules
        for tx in new_txs {
            let tx_hash = tx.hash();
            if let Err(error) = self.add_to_pool(tx) {
                error!(target: "txs_pool", "Failed to add proposed tx {:} to pool, reason: {:?}", tx_hash, error);
            }
        }
    }

    /// NOTE: may remove this method later (currently unused!!!)
    #[cfg(test)]
    pub(crate) fn _resolve_conflict(&mut self, tx: &Transaction) {
        if tx.is_cellbase() {
            return;
        }
        self.pool.resolve_conflict(tx);
    }

    /// Whether the pool is full
    fn is_acceptable(&self) -> Result<(), PoolError> {
        if self.total_size() > self.config.max_pool_size {
            // TODO evict old/large transactions instead
            return Err(PoolError::OverCapacity);
        }
        Ok(())
    }

    // Check that the transaction is not in the pool or chain
    fn check_duplicate(&self, tx: &Transaction) -> Result<(), PoolError> {
        let h = tx.hash();

        {
            if self.pool.contains(tx) || self.orphan.contains(tx) {
                return Err(PoolError::AlreadyInPool);
            }
        }

        if self.shared.contain_transaction(&h) {
            return Err(PoolError::DuplicateOutput);
        }

        Ok(())
    }
}
