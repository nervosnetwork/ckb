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

/// RPC Module Subscription that CKB node will push new messages to subscribers.
///
/// RPC subscriptions require a full duplex connection. CKB offers such connections in the form of
/// TCP (enable with rpc.tcp_listen_address configuration option) and WebSocket (enable with
/// rpc.ws_listen_address).
///
/// ## Examples
///
/// TCP RPC subscription:
///
/// ```text
/// telnet localhost 18114
/// > {"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}
/// < {"jsonrpc":"2.0","result":0,"id":2}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < ...
/// > {"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": [0]}
/// < {"jsonrpc":"2.0","result":true,"id":2}
/// ```
///
/// WebSocket RPC subscription:
///
/// ```javascript
/// let socket = new WebSocket("ws://localhost:28114")
///
/// socket.onmessage = function(event) {
///   console.log(`Data received from server: ${event.data}`);
/// }
///
/// socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}`)
///
/// socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": [0]}`)
/// ```
#[allow(clippy::needless_return)]
#[rpc(server)]
pub trait SubscriptionRpc {
    /// TODO(doc): @doitian
    type Metadata;

    /// Subscribes to a topic.
    ///
    /// ## Params
    ///
    /// * `topic` - Subscription topic (enum: new_tip_header | new_tip_block)
    ///
    /// ## Returns
    ///
    /// This RPC returns the subscription ID as the result. CKB node will push messages in the subscribed
    /// topics to the current RPC connection. The subscript ID is also attached as
    /// `params.subscription` in the push messages.
    ///
    /// Example push message:
    ///
    /// ```json+skip
    /// {
    ///   "jsonrpc": "2.0",
    ///   "method": "subscribe",
    ///   "params": {
    ///     "result": { ... },
    ///     "subscription": "0x2a"
    ///   }
    /// }
    /// ```
    ///
    /// ## Topics
    ///
    /// ### `new_tip_header`
    ///
    /// Whenever there's a block that is appended to the canonical chain, the CKB node will publish the
    /// block header to subscribers.
    ///
    /// The type of the `params.result` in the push message is [`HeaderView`](../../ckb_jsonrpc_types/struct.HeaderView.html).
    ///
    /// ### `new_tip_block`
    ///
    /// Whenever there's a block that is appended to the canonical chain, the CKB node will publish the
    /// whole block to subscribers.
    ///
    /// The type of the `params.result` in the push message is [`BlockView`](../../ckb_jsonrpc_types/struct.BlockView.html).
    ///
    /// ### `new_transaction`
    ///
    /// Subscribers will get notified when a new transaction is submitted to the pool.
    ///
    /// The type of the `params.result` in the push message is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html).
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "subscribe",
    ///   "params": [
    ///     "new_tip_header"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x2a"
    /// }
    /// ```
    #[pubsub(subscription = "subscribe", subscribe, name = "subscribe")]
    fn subscribe(&self, meta: Self::Metadata, subscriber: Subscriber<String>, topic: Topic);

    /// Unsubscribes from a subscribed topic.
    ///
    /// ## Params
    ///
    /// * `id` - Subscription ID
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "unsubscribe",
    ///   "params": [
    ///     "0x2a"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": true
    /// }
    /// ```
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
