#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

extern crate crossbeam_channel;
extern crate fnv;
extern crate nervos_core as core;
extern crate nervos_util as util;

use core::block::IndexedBlock;
use core::transaction::Transaction;
use fnv::FnvHashMap;
use std::sync::Arc;
use util::RwLock;

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    NewTransaction,
    NewHead,
    SwitchFork,
}

pub type Subscriber = crossbeam_channel::Sender<Event>;
pub type Subscribers = FnvHashMap<String, Subscriber>;
pub type CanonSubscriber = crossbeam_channel::Sender<IndexedBlock>;
pub type CanonSubscribers = FnvHashMap<String, CanonSubscriber>;
pub type ForkSubscriber = crossbeam_channel::Sender<(Vec<Transaction>, Vec<Transaction>)>;
pub type ForkSubscribers = FnvHashMap<String, ForkSubscriber>;

#[derive(Clone, Default)]
pub struct Notify {
    pub sync_subscribers: Arc<RwLock<Subscribers>>,
    pub transaction_subscribers: Arc<RwLock<Subscribers>>,
    pub canon_subscribers: Arc<RwLock<CanonSubscribers>>,
    pub fork_subscribers: Arc<RwLock<ForkSubscribers>>,
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

    pub fn register_sync_subscribers<S: ToString>(&self, name: S, sub: Subscriber) {
        self.sync_subscribers.write().insert(name.to_string(), sub);
    }

    pub fn register_canon_subscribers<S: ToString>(&self, name: S, sub: CanonSubscriber) {
        self.canon_subscribers.write().insert(name.to_string(), sub);
    }

    pub fn register_fork_subscribers<S: ToString>(&self, name: S, sub: ForkSubscriber) {
        self.fork_subscribers.write().insert(name.to_string(), sub);
    }

    pub fn notify_sync_head(&self) {
        for sub in self.sync_subscribers.read().values() {
            sub.send(Event::NewHead);
        }
    }

    pub fn notify_new_transaction(&self) {
        for sub in self.transaction_subscribers.read().values() {
            sub.send(Event::NewTransaction);
        }
    }

    pub fn notify_canon_block(&self, b: IndexedBlock) {
        for sub in self.canon_subscribers.read().values() {
            sub.send(b.clone());
        }
    }

    pub fn notify_switch_fork(&self, txs: (Vec<Transaction>, Vec<Transaction>)) {
        for sub in self.fork_subscribers.read().values() {
            sub.send(txs.clone());
        }
    }
}
