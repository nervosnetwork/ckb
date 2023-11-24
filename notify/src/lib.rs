//! TODO(doc): @quake
use ckb_app_config::NotifyConfig;
use ckb_async_runtime::Handle;
use ckb_logger::{debug, error, info, trace};
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_types::packed::Byte32;
use ckb_types::{
    core::{tx_pool::Reject, BlockView},
    packed::Alert,
};
use std::{collections::HashMap, time::Duration};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    oneshot,
};
use tokio::time::timeout;

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

/// watcher request type alias
pub type NotifyWatcher<M> = Sender<Request<String, watch::Receiver<M>>>;

/// Notify timeout config
#[derive(Copy, Clone)]
pub(crate) struct NotifyTimeout {
    pub(crate) tx: Duration,
    pub(crate) alert: Duration,
    pub(crate) script: Duration,
}

const DEFAULT_TX_NOTIFY_TIMEOUT: Duration = Duration::from_millis(300);
const DEFAULT_ALERT_NOTIFY_TIMEOUT: Duration = Duration::from_millis(10_000);
const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_millis(10_000);

impl NotifyTimeout {
    pub(crate) fn new(config: &NotifyConfig) -> Self {
        NotifyTimeout {
            tx: config
                .notify_tx_timeout
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_TX_NOTIFY_TIMEOUT),
            alert: config
                .notify_alert_timeout
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_ALERT_NOTIFY_TIMEOUT),
            script: config
                .script_timeout
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_SCRIPT_TIMEOUT),
        }
    }
}

/// TODO(doc): @quake
#[derive(Clone)]
pub struct NotifyController {
    new_block_register: NotifyRegister<BlockView>,
    new_block_watcher: NotifyWatcher<Byte32>,
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

/// TODO(doc): @quake
pub struct NotifyService {
    config: NotifyConfig,
    new_block_subscribers: HashMap<String, Sender<BlockView>>,
    new_block_watchers: HashMap<String, watch::Sender<Byte32>>,
    new_transaction_subscribers: HashMap<String, Sender<PoolTransactionEntry>>,
    proposed_transaction_subscribers: HashMap<String, Sender<PoolTransactionEntry>>,
    reject_transaction_subscribers: HashMap<String, Sender<(PoolTransactionEntry, Reject)>>,
    network_alert_subscribers: HashMap<String, Sender<Alert>>,
    timeout: NotifyTimeout,
    handle: Handle,
}

impl NotifyService {
    /// TODO(doc): @quake
    pub fn new(config: NotifyConfig, handle: Handle) -> Self {
        let timeout = NotifyTimeout::new(&config);

        Self {
            config,
            new_block_subscribers: HashMap::default(),
            new_block_watchers: HashMap::default(),
            new_transaction_subscribers: HashMap::default(),
            proposed_transaction_subscribers: HashMap::default(),
            reject_transaction_subscribers: HashMap::default(),
            network_alert_subscribers: HashMap::default(),
            timeout,
            handle,
        }
    }

    /// start background tokio spawned task.
    pub fn start(mut self) -> NotifyController {
        let signal_receiver: CancellationToken = new_tokio_exit_rx();
        let handle = self.handle.clone();

        let (new_block_register, mut new_block_register_receiver) =
            mpsc::channel(REGISTER_CHANNEL_SIZE);
        let (new_block_watcher, mut new_block_watcher_receiver) =
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
                    Some(msg) = new_block_register_receiver.recv() => { self.handle_register_new_block(msg) },
                    Some(msg) = new_block_watcher_receiver.recv() => { self.handle_watch_new_block(msg) },
                    Some(msg) = new_block_receiver.recv() => { self.handle_notify_new_block(msg) },
                    Some(msg) = new_transaction_register_receiver.recv() => { self.handle_register_new_transaction(msg) },
                    Some(msg) = new_transaction_receiver.recv() => { self.handle_notify_new_transaction(msg) },
                    Some(msg) = proposed_transaction_register_receiver.recv() => { self.handle_register_proposed_transaction(msg) },
                    Some(msg) = proposed_transaction_receiver.recv() => { self.handle_notify_proposed_transaction(msg) },
                    Some(msg) = reject_transaction_register_receiver.recv() => { self.handle_register_reject_transaction(msg) },
                    Some(msg) = reject_transaction_receiver.recv() => { self.handle_notify_reject_transaction(msg) },
                    Some(msg) = network_alert_register_receiver.recv() => { self.handle_register_network_alert(msg) },
                    Some(msg) = network_alert_receiver.recv() => { self.handle_notify_network_alert(msg) },
                    _ = signal_receiver.cancelled() => {
                        info!("NotifyService received exit signal, exit now");
                        break;
                    }
                    else => break,
                }
            }
        });

        NotifyController {
            new_block_register,
            new_block_watcher,
            new_block_notifier: new_block_sender,
            new_transaction_register,
            new_transaction_notifier: new_transaction_sender,
            proposed_transaction_register,
            proposed_transaction_notifier: proposed_transaction_sender,
            reject_transaction_register,
            reject_transaction_notifier: reject_transaction_sender,
            network_alert_register,
            network_alert_notifier: network_alert_sender,
            handle,
        }
    }

    fn handle_watch_new_block(&mut self, msg: Request<String, watch::Receiver<Byte32>>) {
        let Request {
            responder,
            arguments: name,
        } = msg;
        debug!("Watch new_block {:?}", name);
        let (sender, receiver) = watch::channel(Byte32::zero());
        self.new_block_watchers.insert(name, sender);
        let _ = responder.send(receiver);
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

    fn handle_notify_new_block(&self, block: BlockView) {
        trace!("event new block {:?}", block);
        let block_hash = block.hash();
        // notify all subscribers
        for subscriber in self.new_block_subscribers.values() {
            let block = block.clone();
            let subscriber = subscriber.clone();
            self.handle.spawn(async move {
                if let Err(e) = subscriber.send(block).await {
                    error!("notify new block error {}", e);
                }
            });
        }

        // notify all watchers
        for watcher in self.new_block_watchers.values() {
            if let Err(e) = watcher.send(block_hash.clone()) {
                error!("notify new block watcher error {}", e);
            }
        }

        // notify script
        if let Some(script) = self.config.new_block_notify_script.clone() {
            let script_timeout = self.timeout.script;
            self.handle.spawn(async move {
                let args = [format!("{block_hash:#x}")];
                match timeout(script_timeout, Command::new(&script).args(&args).status()).await {
                    Ok(ret) => match ret {
                        Ok(status) => debug!("the new_block_notify script exited with: {status}"),
                        Err(e) => error!(
                            "failed to run new_block_notify_script: {} {:?}, error: {}",
                            script, args[0], e
                        ),
                    },
                    Err(_) => ckb_logger::warn!("new_block_notify_script {script} timed out"),
                }
            });
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

    fn handle_notify_new_transaction(&self, tx_entry: PoolTransactionEntry) {
        trace!("event new tx {:?}", tx_entry);
        // notify all subscribers
        let tx_timeout = self.timeout.tx;
        // notify all subscribers
        for subscriber in self.new_transaction_subscribers.values() {
            let tx_entry = tx_entry.clone();
            let subscriber = subscriber.clone();
            self.handle.spawn(async move {
                if let Err(e) = subscriber.send_timeout(tx_entry, tx_timeout).await {
                    error!("notify new transaction error {}", e);
                }
            });
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

    fn handle_notify_proposed_transaction(&self, tx_entry: PoolTransactionEntry) {
        trace!("event proposed tx {:?}", tx_entry);
        // notify all subscribers
        let tx_timeout = self.timeout.tx;
        // notify all subscribers
        for subscriber in self.proposed_transaction_subscribers.values() {
            let tx_entry = tx_entry.clone();
            let subscriber = subscriber.clone();
            self.handle.spawn(async move {
                if let Err(e) = subscriber.send_timeout(tx_entry, tx_timeout).await {
                    error!("notify proposed transaction error {}", e);
                }
            });
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

    fn handle_notify_reject_transaction(&self, tx_entry: (PoolTransactionEntry, Reject)) {
        trace!("event reject tx {:?}", tx_entry);
        // notify all subscribers
        let tx_timeout = self.timeout.tx;
        // notify all subscribers
        for subscriber in self.reject_transaction_subscribers.values() {
            let tx_entry = tx_entry.clone();
            let subscriber = subscriber.clone();
            self.handle.spawn(async move {
                if let Err(e) = subscriber.send_timeout(tx_entry, tx_timeout).await {
                    error!("notify reject transaction error {}", e);
                }
            });
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

    fn handle_notify_network_alert(&self, alert: Alert) {
        trace!("event network alert {:?}", alert);
        let alert_timeout = self.timeout.alert;
        let message = alert
            .as_reader()
            .raw()
            .message()
            .as_utf8()
            .expect("alert message should be utf8")
            .to_owned();
        // notify all subscribers
        for subscriber in self.network_alert_subscribers.values() {
            let subscriber = subscriber.clone();
            let alert = alert.clone();
            self.handle.spawn(async move {
                if let Err(e) = subscriber.send_timeout(alert, alert_timeout).await {
                    error!("notify network_alert error {}", e);
                }
            });
        }

        // notify script
        if let Some(script) = self.config.network_alert_notify_script.clone() {
            let script_timeout = self.timeout.script;
            self.handle.spawn(async move {
                let args = [message];
                match timeout(script_timeout, Command::new(&script).args(&args).status()).await {
                    Ok(ret) => match ret {
                        Ok(status) => {
                            debug!("the network_alert_notify script exited with: {}", status)
                        }
                        Err(e) => error!(
                            "failed to run network_alert_notify_script: {} {}, error: {}",
                            script, args[0], e
                        ),
                    },
                    Err(_) => ckb_logger::warn!("network_alert_notify_script {} timed out", script),
                }
            });
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

    /// watch new block notify
    pub async fn watch_new_block<S: ToString>(&self, name: S) -> watch::Receiver<Byte32> {
        Request::call(&self.new_block_watcher, name.to_string())
            .await
            .expect("Watch new block should be OK")
    }

    /// TODO(doc): @quake
    pub fn notify_new_block(&self, block: BlockView) {
        let new_block_notifier = self.new_block_notifier.clone();
        self.handle.spawn(async move {
            if let Err(e) = new_block_notifier.send(block).await {
                error!("notify_new_block channel is closed: {}", e);
            }
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
            if let Err(e) = new_transaction_notifier.send(tx_entry).await {
                error!("notify_new_transaction channel is closed: {}", e);
            }
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
            if let Err(e) = proposed_transaction_notifier.send(tx_entry).await {
                error!("notify_proposed_transaction channel is closed: {}", e);
            }
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
            if let Err(e) = reject_transaction_notifier.send((tx_entry, reject)).await {
                error!("notify_reject_transaction channel is closed: {}", e);
            }
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
            if let Err(e) = network_alert_notifier.send(alert).await {
                error!("notify_network_alert channel is closed: {}", e);
            }
        });
    }
}
