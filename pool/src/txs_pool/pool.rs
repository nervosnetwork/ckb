//! Top-level Pool type, methods, and tests
use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_notify::{Event, ForkTxs, Notify, TXS_POOL_SUBSCRIBER};
use ckb_verification::TransactionVerifier;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::transaction::{
    CellOutput, IndexedTransaction, OutPoint, ProposalShortId, ProposalTransaction,
};
use core::BlockNumber;
use crossbeam_channel;
use fnv::FnvHashSet;
use std::iter;
use std::sync::Arc;
use std::thread;
use txs_pool::types::{
    CandidatePool, CommitPool, InsertionResult, OrphanPool, OutPointStatus, PoolConfig, PoolError,
    ProposalPool,
};
use util::RwLock;

/// The pool itself.
pub struct TransactionPool<T> {
    pub config: PoolConfig,
    /// The candidate pool
    pub candidate: RwLock<CandidatePool>,
    /// The proposal pool
    pub proposal: RwLock<ProposalPool>,
    /// The commit pool
    pub commit: RwLock<CommitPool>,
    /// Orphans in the pool
    pub orphan: RwLock<OrphanPool>,
    // chain will offer to the pool
    pub chain: Arc<T>,

    pub notify: Notify,
}

#[derive(Clone, PartialEq, Debug)]
pub enum PoolCellState {
    /// Cell exists and is the head in its cell chain.
    Head(CellOutput),
    /// Cell in Pool
    Commit(CellOutput),
    /// Cell in Orphan Pool
    Orphan(CellOutput),
    /// Cell exists and is not the head of its cell chain.
    Tail,
    /// Cell does not exist.
    Unknown,
}

impl CellState for PoolCellState {
    fn tail() -> Self {
        PoolCellState::Tail
    }

    fn unknown() -> Self {
        PoolCellState::Unknown
    }

    fn head(&self) -> Option<&CellOutput> {
        match *self {
            PoolCellState::Head(ref output)
            | PoolCellState::Commit(ref output)
            | PoolCellState::Orphan(ref output) => Some(output),
            _ => None,
        }
    }

    fn take_head(self) -> Option<CellOutput> {
        match self {
            PoolCellState::Head(output)
            | PoolCellState::Commit(output)
            | PoolCellState::Orphan(output) => Some(output),
            _ => None,
        }
    }

    fn is_head(&self) -> bool {
        match *self {
            PoolCellState::Head(_) | PoolCellState::Commit(_) | PoolCellState::Orphan(_) => true,
            _ => false,
        }
    }
    fn is_unknown(&self) -> bool {
        match *self {
            PoolCellState::Unknown => true,
            _ => false,
        }
    }
    fn is_tail(&self) -> bool {
        match *self {
            PoolCellState::Tail => true,
            _ => false,
        }
    }
}

impl<T> CellProvider for TransactionPool<T>
where
    T: ChainProvider,
{
    type State = PoolCellState;

    fn cell(&self, o: &OutPoint) -> Self::State {
        if self.commit.read().outpoint_status(o) == OutPointStatus::Spent {
            PoolCellState::Tail
        } else if let Some(output) = self.commit.read().get_output(o) {
            PoolCellState::Commit(output)
        } else if let Some(output) = self.orphan.read().get_output(o) {
            PoolCellState::Orphan(output)
        } else {
            let chain_cell_state = self.chain.cell(o);
            if chain_cell_state.is_head() {
                PoolCellState::Head(chain_cell_state.take_head().expect("state checked"))
            } else if chain_cell_state.is_tail() {
                PoolCellState::Tail
            } else {
                PoolCellState::Unknown
            }
        }
    }

    fn cell_at(&self, _o: &OutPoint, _parent: &H256) -> Self::State {
        unreachable!()
    }
}

impl<T> TransactionPool<T>
where
    T: ChainProvider + 'static,
{
    /// Create a new transaction pool
    pub fn new(config: PoolConfig, chain: Arc<T>, notify: Notify) -> Arc<TransactionPool<T>> {
        let pool = Arc::new(TransactionPool {
            config,
            candidate: RwLock::new(CandidatePool::new()),
            proposal: RwLock::new(ProposalPool::new()),
            commit: RwLock::new(CommitPool::new()),
            orphan: RwLock::new(OrphanPool::new()),
            chain,
            notify,
        });

        let (tx, rx) = crossbeam_channel::unbounded();
        pool.notify
            .register_tip_subscriber(TXS_POOL_SUBSCRIBER, tx.clone());
        pool.notify
            .register_fork_subscriber(TXS_POOL_SUBSCRIBER, tx);
        let pool_cloned = Arc::<TransactionPool<T>>::clone(&pool);
        thread::spawn(move || loop {
            match rx.recv() {
                Some(Event::NewTip(b)) => {
                    pool_cloned.reconcile_block(&b);
                }
                Some(Event::SwitchFork(txs)) => {
                    pool_cloned.switch_fork(&txs);
                }
                None => {
                    info!(target: "txs_pool", "sub channel closed");
                    break;
                }
                event => {
                    warn!(target: "txs_pool", "Unexpected sub message {:?}", event);
                }
            }
        });

        pool
    }

    pub fn switch_fork(&self, txs: &ForkTxs) {
        for tx in txs.old_txs() {
            if tx.is_cellbase() {
                continue;
            }

            self.commit.write().readd_transaction(&tx);
        }

        for tx in txs.new_txs().iter().rev() {
            if tx.is_cellbase() {
                continue;
            }

            let in_pool = { self.commit.write().commit_transaction(&tx) };
            if !in_pool {
                {
                    self.orphan.write().commit_transaction(&tx);
                }

                self.resolve_conflict(&tx);
            }
        }
    }

    /// Get the number of transactions in the pool
    pub fn commit_pool_size(&self) -> usize {
        self.commit.read().size()
    }

    /// Get the number of orphans in the pool
    pub fn orphan_pool_size(&self) -> usize {
        self.orphan.read().size()
    }

    /// Get the number of orphans in the pool
    pub fn candidate_pool_size(&self) -> usize {
        self.candidate.read().size()
    }

    /// Get the total size (transactions + orphans) of the pool
    pub fn total_size(&self) -> usize {
        self.commit_pool_size() + self.orphan_pool_size()
    }

    pub fn insert_candidate(&self, tx: IndexedTransaction) -> bool {
        self.candidate.write().insert(tx)
    }

    pub fn prepare_proposal(&self, n: usize) -> FnvHashSet<ProposalTransaction> {
        self.candidate.read().take(n)
    }

    pub fn query_proposal(
        &self,
        block_number: BlockNumber,
        filter: impl Iterator<Item = ProposalShortId>,
    ) -> Option<(Vec<IndexedTransaction>, Vec<ProposalShortId>)> {
        self.proposal.read().query(block_number, filter)
    }

    pub fn query_proposal_ids(
        &self,
        block_number: BlockNumber,
    ) -> Option<FnvHashSet<ProposalShortId>> {
        self.proposal.read().query_ids(block_number)
    }

    pub fn proposal_n(&self, block_number: BlockNumber, txs: FnvHashSet<ProposalTransaction>) {
        self.candidate.write().update_difference(&txs);
        let clean =
            block_number.saturating_sub(self.chain.consensus().transaction_propagation_time * 2);
        let mut proposal = self.proposal.write();
        proposal.clean(clean);
        proposal.insert(block_number, txs.into_iter());
    }

    pub fn proposal(&self, block_number: BlockNumber, tx: ProposalTransaction) {
        self.candidate.write().remove(&tx);
        let clean =
            block_number.saturating_sub(self.chain.consensus().transaction_propagation_time * 2);
        let mut proposal = self.proposal.write();
        proposal.clean(clean);
        proposal.insert(block_number, iter::once(tx));
    }

    pub fn prepare_commit(
        &self,
        block_number: BlockNumber,
        include: &FnvHashSet<ProposalShortId>,
        n: usize,
    ) -> Vec<IndexedTransaction> {
        let t_prop = self.chain.consensus().transaction_propagation_time;
        let t_timeout = self.chain.consensus().transaction_propagation_timeout;

        // x >= number - t_timeout && x =< block_number - t_prop
        let end = block_number.saturating_sub(t_prop) + 1;
        let start = block_number.saturating_sub(t_timeout);

        let mut pre_commit = { self.proposal.read().take(start..end) };

        pre_commit.retain(|tx| include.contains(&tx.proposal_short_id()));

        for tx in pre_commit {
            let _ = self.add_to_commit_pool(tx.into());
        }

        self.commit.read().get_mineable_transactions(n)
    }

    /// Attempts to add a transaction to the memory pool.
    pub fn add_to_commit_pool(&self, tx: IndexedTransaction) -> Result<InsertionResult, PoolError> {
        // Do we have the capacity to accept this transaction?
        self.is_acceptable()?;

        if tx.is_cellbase() {
            return Err(PoolError::CellBase);
        }

        self.check_duplicate(&tx)?;

        let inputs = tx.input_pts();
        let deps = tx.dep_pts();

        let mut is_orphan = false;
        let mut unknowns = Vec::new();
        let mut conflict = false;

        {
            let rtx = self.resolve_transaction(&tx);

            for (i, cs) in rtx.input_cells.iter().enumerate() {
                if !conflict
                    && self.orphan.read().outpoint_status(&inputs[i]) == OutPointStatus::Spent
                {
                    conflict = true;
                }

                match cs {
                    PoolCellState::Orphan(_) => is_orphan = true,
                    PoolCellState::Unknown => {
                        is_orphan = true;
                        unknowns.push(inputs[i]);
                    }
                    _ => {}
                }

                if is_orphan && conflict {
                    return Err(PoolError::ConflictOrphan);
                }
            }

            for (i, cs) in rtx.dep_cells.iter().enumerate() {
                if self.orphan.read().outpoint_status(&deps[i]) == OutPointStatus::Spent {
                    return Err(PoolError::ConflictOrphan);
                }

                match cs {
                    PoolCellState::Orphan(_) => is_orphan = true,
                    PoolCellState::Unknown => {
                        is_orphan = true;
                        unknowns.push(deps[i]);
                    }
                    _ => {}
                }
            }

            if unknowns.is_empty() {
                TransactionVerifier::new(&rtx)
                    .verify()
                    .map_err(PoolError::InvalidTx)?;
            }
        }

        if is_orphan {
            self.orphan
                .write()
                .add_transaction(tx, unknowns.into_iter());
            return Ok(InsertionResult::Orphan);
        } else {
            if conflict {
                self.orphan.write().resolve_conflict(&tx);
            }

            {
                let mut commit = self.commit.write();
                commit.add_transaction(tx.clone());
            }

            self.notify.notify_new_transaction();
        }

        Ok(InsertionResult::Normal)
    }

    /// Updates the pool and orphan pool with new transactions.
    pub fn reconcile_orphan(&self, tx: &IndexedTransaction) {
        let txs = {
            let mut orphan = self.orphan.write();
            orphan.commit_transaction(tx);
            orphan.get_no_orphan()
        };

        for tx in txs {
            let rtx = self.resolve_transaction(&tx);
            if TransactionVerifier::new(&rtx).verify().is_ok() {
                let mut commit = self.commit.write();
                commit.add_transaction(tx);
            }
        }
    }

    /// Updates the pool with the details of a new block.
    pub fn reconcile_block(&self, b: &IndexedBlock) {
        let txs = &b.commit_transactions;

        for tx in txs {
            if tx.is_cellbase() {
                continue;
            }

            let in_pool = { self.commit.write().commit_transaction(tx) };
            if !in_pool {
                {
                    self.orphan.write().commit_transaction(tx);
                }

                self.resolve_conflict(tx);
            }
        }
    }

    pub fn resolve_conflict(&self, tx: &IndexedTransaction) {
        if tx.is_cellbase() {
            return;
        }

        self.commit.write().resolve_conflict(tx);
        self.orphan.write().resolve_conflict(tx);
    }

    // /// Select a set of mineable transactions for block building.
    // pub fn prepare_mineable_transactions(&self, n: usize) -> Vec<IndexedTransaction> {
    //     self.commit.read().get_mineable_transactions(n)
    // }

    /// Whether the transaction is acceptable to the pool, given both how
    /// full the pool is and the transaction weight.
    fn is_acceptable(&self) -> Result<(), PoolError> {
        if self.total_size() > self.config.max_pool_size {
            // TODO evict old/large transactions instead
            return Err(PoolError::OverCapacity);
        }
        Ok(())
    }

    // Check that the transaction is not in the pool or chain
    fn check_duplicate(&self, tx: &IndexedTransaction) -> Result<(), PoolError> {
        let h = tx.hash();

        {
            if self.commit.read().is_pool_tx(&h) || self.orphan.read().is_pool_tx(&h) {
                return Err(PoolError::AlreadyInPool);
            }
        }

        if self.chain.contain_transaction(&h) {
            return Err(PoolError::DuplicateOutput);
        }

        Ok(())
    }
}
