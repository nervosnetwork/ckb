#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

extern crate crossbeam_channel;
extern crate fnv;
extern crate nervos_util as util;

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

#[derive(Clone, Default)]
pub struct Notify {
    pub sync_subscribers: Arc<RwLock<Subscribers>>,
    pub transaction_subscribers: Arc<RwLock<Subscribers>>,
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

    pub fn notify_sync_head(&self) {
        for sub in self.sync_subscribers.read().values() {
            let _ = sub.send(Event::NewHead);
        }
    }

    pub fn notify_new_transaction(&self) {
        for sub in self.transaction_subscribers.read().values() {
            let _ = sub.send(Event::NewTransaction);
        }
    }
}
