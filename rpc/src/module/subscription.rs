use ckb_channel::select;
use ckb_logger::error;
use ckb_notify::NotifyController;
use jsonrpc_core::{futures::Future, Metadata, Result};
use jsonrpc_derive::rpc;
use jsonrpc_pubsub::{
    typed::{Sink, Subscriber},
    PubSubMetadata, Session, SubscriptionId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    RwLock,
};
use std::thread;

#[derive(Clone, Debug)]
pub struct SubscriptionSession {
    pub(crate) subscription_ids: Arc<RwLock<HashSet<SubscriptionId>>>,
    pub(crate) session: Arc<Session>,
}

impl SubscriptionSession {
    pub fn new(session: Session) -> Self {
        Self {
            subscription_ids: Arc::new(RwLock::new(HashSet::new())),
            session: Arc::new(session),
        }
    }
}

impl Metadata for SubscriptionSession {}

impl PubSubMetadata for SubscriptionSession {
    fn session(&self) -> Option<Arc<Session>> {
        Some(Arc::clone(&self.session))
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    NewTipHeader,
    NewTipBlock,
    NewTransaction,
}

#[allow(clippy::needless_return)]
#[rpc(server)]
pub trait SubscriptionRpc {
    type Metadata;

    #[pubsub(subscription = "subscribe", subscribe, name = "subscribe")]
    fn subscribe(&self, meta: Self::Metadata, subscriber: Subscriber<String>, topic: Topic);

    #[pubsub(subscription = "subscribe", unsubscribe, name = "unsubscribe")]
    fn unsubscribe(&self, meta: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool>;
}

type Subscribers = HashMap<SubscriptionId, Sink<String>>;

#[derive(Default)]
pub struct SubscriptionRpcImpl {
    pub(crate) id_generator: AtomicUsize,
    pub(crate) subscribers: Arc<RwLock<HashMap<Topic, Subscribers>>>,
}

impl SubscriptionRpc for SubscriptionRpcImpl {
    type Metadata = Option<SubscriptionSession>;

    fn subscribe(&self, meta: Self::Metadata, subscriber: Subscriber<String>, topic: Topic) {
        if let Some(session) = meta {
            let id = SubscriptionId::String(format!(
                "{:#x}",
                self.id_generator.fetch_add(1, Ordering::SeqCst)
            ));
            if let Ok(sink) = subscriber.assign_id(id.clone()) {
                let mut subscribers = self
                    .subscribers
                    .write()
                    .expect("acquiring subscribers write lock");
                subscribers
                    .entry(topic)
                    .or_default()
                    .insert(id.clone(), sink);

                session
                    .subscription_ids
                    .write()
                    .expect("acquiring subscription_ids write lock")
                    .insert(id);
            }
        }
    }

    fn unsubscribe(&self, meta: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool> {
        let mut subscribers = self
            .subscribers
            .write()
            .expect("acquiring subscribers write lock");
        match meta {
            // unsubscribe handler method is explicitly called.
            Some(Some(session)) => {
                if session
                    .subscription_ids
                    .write()
                    .expect("acquiring subscription_ids write lock")
                    .remove(&id)
                {
                    Ok(subscribers.values_mut().any(|s| s.remove(&id).is_some()))
                } else {
                    Ok(false)
                }
            }
            // closed or dropped connection
            _ => {
                subscribers.values_mut().for_each(|s| {
                    s.remove(&id);
                });
                Ok(true)
            }
        }
    }
}

impl SubscriptionRpcImpl {
    // remove `allow` tag when https://github.com/crossbeam-rs/crossbeam/issues/404 is solved
    #[allow(clippy::zero_ptr, clippy::drop_copy)]
    pub fn new<S: ToString>(notify_controller: NotifyController, name: S) -> Self {
        let new_block_receiver = notify_controller.subscribe_new_block(name.to_string());
        let new_transaction_receiver =
            notify_controller.subscribe_new_transaction(name.to_string());

        let subscription_rpc_impl = SubscriptionRpcImpl::default();
        let subscribers = Arc::clone(&subscription_rpc_impl.subscribers);

        let thread_builder = thread::Builder::new().name(name.to_string());
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(new_block_receiver) -> msg => match msg {
                        Ok(block) => {
                            let subscribers = subscribers.read().expect("acquiring subscribers read lock");
                            if let Some(new_tip_header_subscribers) = subscribers.get(&Topic::NewTipHeader) {
                                let header: ckb_jsonrpc_types::HeaderView  = block.header().into();
                                let json_string = Ok(serde_json::to_string(&header).expect("serialization should be ok"));
                                for sink in new_tip_header_subscribers.values() {
                                    let _ = sink.notify(json_string.clone()).wait();
                                }
                            }
                            if let Some(new_tip_block_subscribers) = subscribers.get(&Topic::NewTipBlock) {
                                let block: ckb_jsonrpc_types::BlockView  = block.into();
                                let json_string = Ok(serde_json::to_string(&block).expect("serialization should be ok"));
                                for sink in new_tip_block_subscribers.values() {
                                    let _ = sink.notify(json_string.clone()).wait();
                                }
                            }
                        },
                        _ => {
                            error!("new_block_receiver closed");
                            break;
                        },
                    },
                    recv(new_transaction_receiver) -> msg => match msg {
                        Ok(tx_entry) => {
                            let subscribers = subscribers.read().expect("acquiring subscribers read lock");
                            if let Some(new_transaction_subscribers) = subscribers.get(&Topic::NewTransaction) {
                                let tx_entry = ckb_jsonrpc_types::PoolTransactionEntry {
                                    transaction: tx_entry.transaction.into(),
                                    cycles: tx_entry.cycles.into(),
                                    size: (tx_entry.size as u64).into(),
                                    fee: tx_entry.fee.into(),
                                };
                                let json_string = Ok(serde_json::to_string(&tx_entry).expect("serialization should be ok"));
                                for sink in new_transaction_subscribers.values() {
                                    let _ = sink.notify(json_string.clone()).wait();
                                }
                            }
                        },
                        _ => {
                            error!("new_transaction_receiver closed");
                            break;
                        },
                    },
                }
            })
            .expect("Start SubscriptionRpc thread failed");

        subscription_rpc_impl
    }
}
