#![allow(missing_docs)]

use async_trait::async_trait;
use ckb_async_runtime::Handle;
use ckb_jsonrpc_types::Topic;
use ckb_notify::NotifyController;
use futures_util::{stream::BoxStream, Stream};
use jsonrpc_core::Result;
use jsonrpc_utils::{pub_sub::PublishMsg, rpc};
use tokio::sync::broadcast;

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
/// < {"jsonrpc":"2.0","result":"0x0","id":2}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
///"subscription":0}}
/// < ...
/// > {"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": ["0x0"]}
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
/// socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": ["0x0"]}`)
/// ```
///
///

#[allow(clippy::needless_return)]
#[rpc]
#[async_trait]
pub trait SubscriptionRpc {
    /// Context to implement the subscription RPC.
    /// type Metadata;

    /// Subscribes to a topic.
    ///
    /// ## Params
    ///
    /// * `topic` - Subscription topic (enum: new_tip_header | new_tip_block | new_transaction | proposed_transaction | rejected_transaction)
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
    /// ### `proposed_transaction`
    ///
    /// Subscribers will get notified when an in-pool transaction is proposed by chain.
    ///
    /// The type of the `params.result` in the push message is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html).
    ///
    /// ### `rejected_transaction`
    ///
    /// Subscribers will get notified when a pending transaction is rejected by tx-pool.
    ///
    /// The type of the `params.result` in the push message is an array contain:
    ///
    /// The type of the `params.result` in the push message is a two-elements array, where
    ///
    /// -   the first item type is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html), and
    /// -   the second item type is [`PoolTransactionReject`](../../ckb_jsonrpc_types/struct.PoolTransactionReject.html).
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
    ///   "result": "0xf3"
    /// }
    /// ```
    /// Unsubscribe Request
    ///
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "unsubscribe",
    ///   "params": [
    ///     "0xf3"
    ///   ]
    /// }
    ///
    ///
    type S: Stream<Item = PublishMsg<String>> + Send + 'static;
    #[rpc(pub_sub(notify = "subscribe", unsubscribe = "unsubscribe"))]
    fn subscribe(&self, topic: Topic) -> Result<Self::S>;
}

#[derive(Clone)]
pub struct SubscriptionRpcImpl {
    pub new_tip_header_sender: broadcast::Sender<PublishMsg<String>>,
    pub new_tip_block_sender: broadcast::Sender<PublishMsg<String>>,
    pub new_transaction_sender: broadcast::Sender<PublishMsg<String>>,
    pub proposed_transaction_sender: broadcast::Sender<PublishMsg<String>>,
    pub new_reject_transaction_sender: broadcast::Sender<PublishMsg<String>>,
}

macro_rules! publiser_send {
    ($ty:ty, $info:expr, $sender:ident) => {{
        let msg: $ty = $info.into();
        let json_string = serde_json::to_string(&msg).expect("serialization should be ok");
        drop($sender.send(PublishMsg::result(&json_string)));
    }};
}

#[async_trait]
impl SubscriptionRpc for SubscriptionRpcImpl {
    type S = BoxStream<'static, PublishMsg<String>>;
    fn subscribe(&self, topic: Topic) -> Result<Self::S> {
        let tx = match topic {
            Topic::NewTipHeader => self.new_tip_header_sender.clone(),
            Topic::NewTipBlock => self.new_tip_block_sender.clone(),
            Topic::NewTransaction => self.new_transaction_sender.clone(),
            Topic::ProposedTransaction => self.proposed_transaction_sender.clone(),
            Topic::RejectedTransaction => self.new_reject_transaction_sender.clone(),
        };
        Ok(Box::pin(async_stream::stream! {
               while let Ok(msg) = tx.clone().subscribe().recv().await {
                    yield msg;
               }
        }))
    }
}

impl SubscriptionRpcImpl {
    pub async fn new(notify_controller: NotifyController, handle: Handle) -> Self {
        const SUBSCRIBER_NAME: &str = "TcpSubscription";

        let mut new_block_receiver = notify_controller
            .subscribe_new_block(SUBSCRIBER_NAME.to_string())
            .await;
        let mut new_transaction_receiver = notify_controller
            .subscribe_new_transaction(SUBSCRIBER_NAME.to_string())
            .await;
        let mut proposed_transaction_receiver = notify_controller
            .subscribe_proposed_transaction(SUBSCRIBER_NAME.to_string())
            .await;
        let mut reject_transaction_receiver = notify_controller
            .subscribe_reject_transaction(SUBSCRIBER_NAME.to_string())
            .await;

        let (new_tip_header_sender, _) = broadcast::channel(10);
        let (new_tip_block_sender, _) = broadcast::channel(10);
        let (proposed_transaction_sender, _) = broadcast::channel(10);
        let (new_transaction_sender, _) = broadcast::channel(10);
        let (new_reject_transaction_sender, _) = broadcast::channel(10);

        handle.spawn({
            let new_tip_header_sender = new_tip_header_sender.clone();
            let new_tip_block_sender = new_tip_block_sender.clone();
            let new_transaction_sender = new_transaction_sender.clone();
            let proposed_transaction_sender = proposed_transaction_sender.clone();
            let new_reject_transaction_sender = new_reject_transaction_sender.clone();
            async move {
            loop {
                tokio::select! {
                    Some(block) = new_block_receiver.recv() => {
                        publiser_send!(ckb_jsonrpc_types::HeaderView, block.header(), new_tip_header_sender);
                        publiser_send!(ckb_jsonrpc_types::BlockView, block, new_tip_block_sender);
                    },
                    Some(tx_entry) = new_transaction_receiver.recv() => {
                        publiser_send!(ckb_jsonrpc_types::PoolTransactionEntry, tx_entry, new_transaction_sender);
                    },
                    Some(tx_entry) = proposed_transaction_receiver.recv() => {
                        publiser_send!(ckb_jsonrpc_types::PoolTransactionEntry, tx_entry, proposed_transaction_sender);
                    },
                    Some((tx_entry, reject)) = reject_transaction_receiver.recv() => {
                        publiser_send!((ckb_jsonrpc_types::PoolTransactionEntry, ckb_jsonrpc_types::PoolTransactionReject),
                                        (tx_entry.into(), reject.into()),
                                        new_reject_transaction_sender);
                    }
                    else => break,
                }
            }
        }});

        Self {
            new_tip_header_sender,
            new_tip_block_sender,
            new_transaction_sender,
            proposed_transaction_sender,
            new_reject_transaction_sender,
        }
    }
}
