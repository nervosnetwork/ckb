//! TODO(doc): @quake
use ckb_app_config::NotifyConfig;
use ckb_async_runtime::Handle;
use ckb_logger::{debug, error, trace};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{
    core::{tx_pool::Reject, BlockView},
    packed::Alert,
};
use std::collections::HashMap;
use std::process::Command;
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    oneshot,
};

pub use ckb_types::core::service::PoolTransactionEntry;

/// Asynchronous request sent to the service.
pub struct Request<A, R> {
    /// Oneshot channel for the service to send back the response.
    pub responder: oneshot::Sender<R>,
    /// Request arguments.
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    /// Call the service with the arguments and wait for the response.
    pub async fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responder, response) = oneshot::channel();
        let _ = sender
            .send(Request {
                responder,
                arguments,
            })
            .await;
        response.await.ok()
    }
}

/// TODO(doc): @quake
pub const SIGNAL_CHANNEL_SIZE: usize = 1;
/// TODO(doc): @quake
pub const REGISTER_CHANNEL_SIZE: usize = 2;
/// TODO(doc): @quake
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

/// TODO(doc): @quake
pub type NotifyRegister<M> = Sender<Request<String, Receiver<M>>>;

/// TODO(doc): @quake
#[derive(Clone)]
pub struct NotifyController {
    stop: StopHandler<()>,
    new_block_register: NotifyRegister<BlockView>,
    new_block_notifier: Sender<BlockView>,
    new_transaction_register: NotifyRegister<PoolTransactionEntry>,
    new_transaction_notifier: Sender<PoolTransactionEntry>,
    proposed_transaction_register: NotifyRegister<PoolTransactionEntry>,
    proposed_transaction_notifier: Sender<PoolTransactionEntry>,
    reject_transaction_register: NotifyRegister<(PoolTransactionEntry, Reject)>,
    reject_transaction_notifier: Sender<(PoolTransactionEntry, Reject)>,
    network_alert_register: NotifyRegister<Alert>,
    network_alert_notifier: Sender<Alert>,
    handle: Handle,
}

impl Drop for NotifyController {
    fn drop(&mut self) {
        self.stop.try_send(());
    }
}

/// TODO(doc): @quake
pub struct NotifyService {
    config: NotifyConfig,
    new_block_subscribers: HashMap<String, Sender<BlockView>>,
    new_transaction_subscribers: HashMap<String, Sender<PoolTransactionEntry>>,
    proposed_transaction_subscribers: HashMap<String, Sender<PoolTransactionEntry>>,
    reject_transaction_subscribers: HashMap<String, Sender<(PoolTransactionEntry, Reject)>>,
    network_alert_subscribers: HashMap<String, Sender<Alert>>,
}

impl NotifyService {
    /// TODO(doc): @quake
    pub fn new(config: NotifyConfig) -> Self {
        Self {
            config,
            new_block_subscribers: HashMap::default(),
            new_transaction_subscribers: HashMap::default(),
            proposed_transaction_subscribers: HashMap::default(),
            reject_transaction_subscribers: HashMap::default(),
            network_alert_subscribers: HashMap::default(),
        }
    }

    /// start background tokio spawned task.
    pub fn start(mut self, handle: Handle) -> NotifyController {
        let (signal_sender, mut signal_receiver) = oneshot::channel();

        let (new_block_register, mut new_block_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (new_block_sender, mut new_block_receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);

        let (new_transaction_register, mut new_transaction_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (new_transaction_sender, mut new_transaction_receiver) =
            mpsc::channel(NOTIFY_CHANNEL_SIZE);

        let (proposed_transaction_register, mut proposed_transaction_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (proposed_transaction_sender, mut proposed_transaction_receiver) =
            mpsc::channel(NOTIFY_CHANNEL_SIZE);

        let (reject_transaction_register, mut reject_transaction_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (reject_transaction_sender, mut reject_transaction_receiver) =
            mpsc::channel(NOTIFY_CHANNEL_SIZE);

        let (network_alert_register, mut network_alert_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (network_alert_sender, mut network_alert_receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);

        handle.spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut signal_receiver => {
                        break;
                    }
                    Some(msg) = new_block_register_receiver.recv() => { self.handle_register_new_block(msg) },
                    Some(msg) = new_block_receiver.recv() => { self.handle_notify_new_block(msg).await },
                    Some(msg) = new_transaction_register_receiver.recv() => { self.handle_register_new_transaction(msg) },
                    Some(msg) = new_transaction_receiver.recv() => { self.handle_notify_new_transaction(msg).await },
                    Some(msg) = proposed_transaction_register_receiver.recv() => { self.handle_register_proposed_transaction(msg) },
                    Some(msg) = proposed_transaction_receiver.recv() => { self.handle_notify_proposed_transaction(msg).await },
                    Some(msg) = reject_transaction_register_receiver.recv() => { self.handle_register_reject_transaction(msg) },
                    Some(msg) = reject_transaction_receiver.recv() => { self.handle_notify_reject_transaction(msg).await },
                    Some(msg) = network_alert_register_receiver.recv() => { self.handle_register_network_alert(msg) },
                    Some(msg) = network_alert_receiver.recv() => { self.handle_notify_network_alert(msg).await },
                    else => break,
                }
            }
        });

        NotifyController {
            new_block_register,
            new_block_notifier: new_block_sender,
            new_transaction_register,
            new_transaction_notifier: new_transaction_sender,
            proposed_transaction_register,
            proposed_transaction_notifier: proposed_transaction_sender,
            reject_transaction_register,
            reject_transaction_notifier: reject_transaction_sender,
            network_alert_register,
            network_alert_notifier: network_alert_sender,
            stop: StopHandler::new(
                SignalSender::Tokio(signal_sender),
                None,
                "notify".to_string(),
            ),
            handle,
        }
    }

    fn handle_register_new_block(&mut self, msg: Request<String, Receiver<BlockView>>) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Register new_block {:?}", name);
        let (sender, receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);
        self.new_block_subscribers.insert(name, sender);
        let _ = responder.send(receiver);
    }

    async fn handle_notify_new_block(&mut self, block: BlockView) {
        trace!("event new block {:?}", block);
        // notify all subscribers
        for subscriber in self.new_block_subscribers.values() {
            let _ = subscriber.send(block.clone()).await;
        }
        // notify script
        if let Some(script) = self.config.new_block_notify_script.as_ref() {
            let args = [format!("{:#x}", block.hash())];
            if let Err(err) = Command::new(script).args(&args).status() {
                error!(
                    "failed to run new_block_notify_script: {} {}, error: {}",
                    script, args[0], err
                );
            }
        }
    }

    fn handle_register_new_transaction(
        &mut self,
        msg: Request<String, Receiver<PoolTransactionEntry>>,
    ) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Register new_transaction {:?}", name);
        let (sender, receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);
        self.new_transaction_subscribers.insert(name, sender);
        let _ = responder.send(receiver);
    }

    async fn handle_notify_new_transaction(&mut self, tx_entry: PoolTransactionEntry) {
        trace!("event new tx {:?}", tx_entry);
        // notify all subscribers
        for subscriber in self.new_transaction_subscribers.values() {
            let _ = subscriber.send(tx_entry.clone()).await;
        }
    }

    fn handle_register_proposed_transaction(
        &mut self,
        msg: Request<String, Receiver<PoolTransactionEntry>>,
    ) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Register proposed_transaction {:?}", name);
        let (sender, receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);
        self.proposed_transaction_subscribers.insert(name, sender);
        let _ = responder.send(receiver);
    }

    async fn handle_notify_proposed_transaction(&mut self, tx_entry: PoolTransactionEntry) {
        trace!("event proposed tx {:?}", tx_entry);
        // notify all subscribers
        for subscriber in self.proposed_transaction_subscribers.values() {
            let _ = subscriber.send(tx_entry.clone()).await;
        }
    }

    fn handle_register_reject_transaction(
        &mut self,
        msg: Request<String, Receiver<(PoolTransactionEntry, Reject)>>,
    ) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Register reject_transaction {:?}", name);
        let (sender, receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);
        self.reject_transaction_subscribers.insert(name, sender);
        let _ = responder.send(receiver);
    }

    async fn handle_notify_reject_transaction(&mut self, tx_entry: (PoolTransactionEntry, Reject)) {
        trace!("event reject tx {:?}", tx_entry);
        // notify all subscribers
        for subscriber in self.reject_transaction_subscribers.values() {
            let _ = subscriber.send(tx_entry.clone()).await;
        }
    }

    fn handle_register_network_alert(&mut self, msg: Request<String, Receiver<Alert>>) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Register network_alert {:?}", name);
        let (sender, receiver) = mpsc::channel(NOTIFY_CHANNEL_SIZE);
        self.network_alert_subscribers.insert(name, sender);
        let _ = responder.send(receiver);
    }

    async fn handle_notify_network_alert(&mut self, alert: Alert) {
        trace!("event network alert {:?}", alert);
        // notify all subscribers
        for subscriber in self.network_alert_subscribers.values() {
            let _ = subscriber.send(alert.clone()).await;
        }
        // notify script
        if let Some(script) = self.config.network_alert_notify_script.as_ref() {
            let args = [alert
                .as_reader()
                .raw()
                .message()
                .as_utf8()
                .expect("alert message should be utf8")
                .to_owned()];
            if let Err(err) = Command::new(script).args(&args).status() {
                error!(
                    "failed to run network_alert_notify_script: {} {}, error: {}",
                    script, args[0], err
                );
            }
        }
    }
}

impl NotifyController {
    /// TODO(doc): @quake
    pub async fn subscribe_new_block<S: ToString>(&self, name: S) -> Receiver<BlockView> {
        Request::call(&self.new_block_register, name.to_string())
            .await
            .expect("Subscribe new block should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_new_block(&self, block: BlockView) {
        let new_block_notifier = self.new_block_notifier.clone();
        self.handle.spawn(async move {
            let _ = new_block_notifier.send(block).await;
        });
    }

    /// TODO(doc): @quake
    pub async fn subscribe_new_transaction<S: ToString>(
        &self,
        name: S,
    ) -> Receiver<PoolTransactionEntry> {
        Request::call(&self.new_transaction_register, name.to_string())
            .await
            .expect("Subscribe new transaction should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_new_transaction(&self, tx_entry: PoolTransactionEntry) {
        let new_transaction_notifier = self.new_transaction_notifier.clone();
        self.handle.spawn(async move {
            let _ = new_transaction_notifier.send(tx_entry).await;
        });
    }

    /// TODO(doc): @quake
    pub async fn subscribe_proposed_transaction<S: ToString>(
        &self,
        name: S,
    ) -> Receiver<PoolTransactionEntry> {
        Request::call(&self.proposed_transaction_register, name.to_string())
            .await
            .expect("Subscribe proposed transaction should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_proposed_transaction(&self, tx_entry: PoolTransactionEntry) {
        let proposed_transaction_notifier = self.proposed_transaction_notifier.clone();
        self.handle.spawn(async move {
            let _ = proposed_transaction_notifier.send(tx_entry).await;
        });
    }

    /// TODO(doc): @quake
    pub async fn subscribe_reject_transaction<S: ToString>(
        &self,
        name: S,
    ) -> Receiver<(PoolTransactionEntry, Reject)> {
        Request::call(&self.reject_transaction_register, name.to_string())
            .await
            .expect("Subscribe rejected transaction should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_reject_transaction(&self, tx_entry: PoolTransactionEntry, reject: Reject) {
        let reject_transaction_notifier = self.reject_transaction_notifier.clone();
        self.handle.spawn(async move {
            let _ = reject_transaction_notifier.send((tx_entry, reject)).await;
        });
    }

    /// TODO(doc): @quake
    pub async fn subscribe_network_alert<S: ToString>(&self, name: S) -> Receiver<Alert> {
        Request::call(&self.network_alert_register, name.to_string())
            .await
            .expect("Subscribe network alert should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_network_alert(&self, alert: Alert) {
        let network_alert_notifier = self.network_alert_notifier.clone();
        self.handle.spawn(async move {
            let _ = network_alert_notifier.send(alert).await;
        });
    }
}
