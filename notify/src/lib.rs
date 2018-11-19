#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

extern crate ckb_core as core;
extern crate ckb_util as util;
extern crate crossbeam_channel;
extern crate fnv;

use core::block::IndexedBlock;
use core::transaction::Transaction;
use fnv::FnvHashMap;
use std::sync::Arc;
use util::RwLock;

pub type OldTxs = Vec<Transaction>;
pub type NewTxs = Vec<Transaction>;

pub const MINER_SUBSCRIBER: &str = "miner";
pub const TXS_POOL_SUBSCRIBER: &str = "txs_pool";

#[derive(Clone, PartialEq, Debug, Default)]
pub struct ForkTxs(pub OldTxs, pub NewTxs);

impl ForkTxs {
    pub fn old_txs(&self) -> &Vec<Transaction> {
        &self.0
    }

    pub fn new_txs(&self) -> &Vec<Transaction> {
        &self.1
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    NewTransaction,
    NewTip(Arc<IndexedBlock>),
    SideChainBlock(Arc<IndexedBlock>),
    SwitchFork(Arc<ForkTxs>),
}

pub type Subscriber = crossbeam_channel::Sender<Event>;
pub type Subscribers = FnvHashMap<String, Subscriber>;

#[derive(Clone, Default, Debug)]
pub struct Notify {
    pub tip_subscribers: Arc<RwLock<Subscribers>>,
    pub transaction_subscribers: Arc<RwLock<Subscribers>>,
    pub side_chain_subscribers: Arc<RwLock<Subscribers>>,
    pub fork_subscribers: Arc<RwLock<Subscribers>>,
}

impl Notify {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_transaction_subscriber<S: ToString>(&self, name: S, sub: Subscriber) {
        self.transaction_subscribers
            .write()
            .insert(name.to_string(), sub);
    }

    pub fn register_tip_subscriber<S: ToString>(&self, name: S, sub: Subscriber) {
        self.tip_subscribers.write().insert(name.to_string(), sub);
    }

    pub fn register_fork_subscriber<S: ToString>(&self, name: S, sub: Subscriber) {
        self.fork_subscribers.write().insert(name.to_string(), sub);
    }

    pub fn register_side_chain_subscriber<S: ToString>(&self, name: S, sub: Subscriber) {
        self.side_chain_subscribers
            .write()
            .insert(name.to_string(), sub);
    }

    pub fn notify_new_tip<F>(&self, block: &IndexedBlock, filter: Option<F>)
    where
        F: Fn(&str) -> bool,
    {
        let block = Arc::new(block.clone());
        if let Some(filter) = filter {
            for (_, sub) in self
                .tip_subscribers
                .read()
                .iter()
                .filter(|(name, _sub)| filter(name))
            {
                sub.send(Event::NewTip(Arc::clone(&block)));
            }
        } else {
            for sub in self.tip_subscribers.read().values() {
                sub.send(Event::NewTip(Arc::clone(&block)));
            }
        }
    }

    pub fn notify_new_transaction<F>(&self, filter: Option<F>)
    where
        F: Fn(&str) -> bool,
    {
        if let Some(filter) = filter {
            for (_, sub) in self
                .transaction_subscribers
                .read()
                .iter()
                .filter(|(name, _sub)| filter(name))
            {
                sub.send(Event::NewTransaction);
            }
        } else {
            for sub in self.transaction_subscribers.read().values() {
                sub.send(Event::NewTransaction);
            }
        }
    }

    pub fn notify_side_chain_block<F>(&self, block: &IndexedBlock, filter: Option<F>)
    where
        F: Fn(&str) -> bool,
    {
        let block = Arc::new(block.clone());

        if let Some(filter) = filter {
            for (_, sub) in self
                .side_chain_subscribers
                .read()
                .iter()
                .filter(|(name, _sub)| filter(name))
            {
                sub.send(Event::SideChainBlock(Arc::clone(&block)));
            }
        } else {
            for sub in self.side_chain_subscribers.read().values() {
                sub.send(Event::SideChainBlock(Arc::clone(&block)));
            }
        }
    }

    pub fn notify_switch_fork<F>(&self, txs: ForkTxs, filter: Option<F>)
    where
        F: Fn(&str) -> bool,
    {
        let txs = Arc::new(txs);

        if let Some(filter) = filter {
            for (_, sub) in self
                .fork_subscribers
                .read()
                .iter()
                .filter(|(name, _sub)| filter(name))
            {
                sub.send(Event::SwitchFork(Arc::clone(&txs)));
            }
        } else {
            for sub in self.fork_subscribers.read().values() {
                sub.send(Event::SwitchFork(Arc::clone(&txs)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclude_miner_sub(name: &str) -> bool {
        name != MINER_SUBSCRIBER
    }

    #[test]
    fn test_transaction() {
        let notify = Notify::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.notify_new_transaction::<fn(&str) -> bool>(None);
        assert_eq!(rx.try_recv(), Some(Event::NewTransaction));
        notify.notify_new_transaction(Some(exclude_miner_sub));
        assert_eq!(rx.try_recv(), None);
    }

    #[test]
    fn test_new_tip() {
        let notify = Notify::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let tip = Arc::new(IndexedBlock::default());

        notify.register_tip_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.notify_new_tip::<fn(&str) -> bool>(&tip, None);
        assert_eq!(rx.try_recv(), Some(Event::NewTip(Arc::clone(&tip))));
        notify.notify_new_tip(&tip, Some(exclude_miner_sub));
        assert_eq!(rx.try_recv(), None);
    }

    #[test]
    fn test_side_chain_block() {
        let notify = Notify::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let tip = Arc::new(IndexedBlock::default());

        notify.register_side_chain_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.notify_side_chain_block::<fn(&str) -> bool>(&tip, None);
        assert_eq!(rx.try_recv(), Some(Event::SideChainBlock(Arc::clone(&tip))));
        notify.notify_side_chain_block(&tip, Some(exclude_miner_sub));
        assert_eq!(rx.try_recv(), None);
    }

    #[test]
    fn test_switch_fork() {
        let notify = Notify::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let txs = ForkTxs::default();

        notify.register_fork_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.notify_switch_fork::<fn(&str) -> bool>(txs.clone(), None);
        assert_eq!(
            rx.try_recv(),
            Some(Event::SwitchFork(Arc::new(txs.clone())))
        );
        notify.notify_switch_fork(txs, Some(exclude_miner_sub));
        assert_eq!(rx.try_recv(), None);
    }
}
