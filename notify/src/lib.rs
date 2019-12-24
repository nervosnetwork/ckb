use ckb_logger::{debug, error, trace};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{
    core::{service::Request, BlockView},
    packed::Alert,
};
use crossbeam_channel::{bounded, select, Receiver, RecvError, Sender};
use serde::{Deserialize, Serialize};
use serde_json::to_string;
use std::collections::HashMap;
use std::process::Command;
use std::thread;

pub const SIGNAL_CHANNEL_SIZE: usize = 1;
pub const REGISTER_CHANNEL_SIZE: usize = 2;
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

pub type NotifyRegister<M> = Sender<Request<String, Receiver<M>>>;

#[derive(Clone)]
pub struct NotifyController {
    stop: StopHandler<()>,
    new_block_register: NotifyRegister<BlockView>,
    new_block_notifier: Sender<BlockView>,
    network_alert_register: NotifyRegister<Alert>,
    network_alert_notifier: Sender<Alert>,
}

impl Drop for NotifyController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

pub struct NotifyService {
    config: Config,
    new_block_subscribers: HashMap<String, Sender<BlockView>>,
    network_alert_subscribers: HashMap<String, Sender<Alert>>,
}

impl NotifyService {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            new_block_subscribers: HashMap::default(),
            network_alert_subscribers: HashMap::default(),
        }
    }

    // remove `allow` tag when https://github.com/crossbeam-rs/crossbeam/issues/404 is solved
    #[allow(clippy::zero_ptr, clippy::drop_copy)]
    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> NotifyController {
        let (signal_sender, signal_receiver) = bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (new_block_register, new_block_register_receiver) = bounded(REGISTER_CHANNEL_SIZE);
        let (new_block_sender, new_block_receiver) = bounded::<BlockView>(NOTIFY_CHANNEL_SIZE);
        let (network_alert_register, network_alert_register_receiver) =
            bounded(REGISTER_CHANNEL_SIZE);
        let (network_alert_sender, network_alert_receiver) = bounded::<Alert>(NOTIFY_CHANNEL_SIZE);

        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let join_handle = thread_builder
            .spawn(move || loop {
                select! {
                    recv(signal_receiver) -> _ => {
                        break;
                    }
                    recv(new_block_register_receiver) -> msg => self.handle_register_new_block(msg),
                    recv(new_block_receiver) -> msg => self.handle_notify_new_block(msg),
                    recv(network_alert_register_receiver) -> msg => self.handle_register_network_alert(msg),
                    recv(network_alert_receiver) -> msg => self.handle_notify_network_alert(msg),
                }
            })
            .expect("Start notify service failed");

        NotifyController {
            new_block_register,
            new_block_notifier: new_block_sender,
            network_alert_register,
            network_alert_notifier: network_alert_sender,
            stop: StopHandler::new(SignalSender::Crossbeam(signal_sender), join_handle),
        }
    }

    fn handle_register_new_block(
        &mut self,
        msg: Result<Request<String, Receiver<BlockView>>, RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: name,
            }) => {
                debug!("Register new_block {:?}", name);
                let (sender, receiver) = bounded::<BlockView>(NOTIFY_CHANNEL_SIZE);
                self.new_block_subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => debug!("Register new_block channel is closed"),
        }
    }

    fn handle_notify_new_block(&mut self, msg: Result<BlockView, RecvError>) {
        match msg {
            Ok(block) => {
                trace!("event new block {:?}", block);
                // notify all subscribers
                for subscriber in self.new_block_subscribers.values() {
                    let _ = subscriber.send(block.clone());
                }
                // notify script
                if let Some(script) = self.config.new_block_notify_script.as_ref() {
                    let block_json: ckb_jsonrpc_types::BlockView = block.into();
                    let args = [to_string(&block_json).expect("serializing should be ok")];
                    if let Err(err) = Command::new(script).args(&args).status() {
                        error!(
                            "failed to run new_block_notify_script script, error: {}",
                            err
                        );
                    }
                }
            }
            _ => debug!("new block channel is closed"),
        }
    }

    fn handle_register_network_alert(
        &mut self,
        msg: Result<Request<String, Receiver<Alert>>, RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: name,
            }) => {
                debug!("Register network_alert {:?}", name);
                let (sender, receiver) = bounded::<Alert>(NOTIFY_CHANNEL_SIZE);
                self.network_alert_subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => debug!("Register network_alert channel is closed"),
        }
    }

    fn handle_notify_network_alert(&mut self, msg: Result<Alert, RecvError>) {
        match msg {
            Ok(alert) => {
                trace!("event network alert {:?}", alert);
                // notify all subscribers
                for subscriber in self.network_alert_subscribers.values() {
                    let _ = subscriber.send(alert.clone());
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
                            "failed to run network_alert_notify_script script, error: {}",
                            err
                        );
                    }
                }
            }
            _ => debug!("network alert channel is closed"),
        }
    }
}

impl NotifyController {
    pub fn subscribe_new_block<S: ToString>(&self, name: S) -> Receiver<BlockView> {
        Request::call(&self.new_block_register, name.to_string())
            .expect("Subscribe new block should be OK")
    }

    pub fn notify_new_block(&self, block: BlockView) {
        let _ = self.new_block_notifier.send(block);
    }

    pub fn subscribe_network_alert<S: ToString>(&self, name: S) -> Receiver<Alert> {
        Request::call(&self.network_alert_register, name.to_string())
            .expect("Subscribe network alert should be OK")
    }

    pub fn notify_network_alert(&self, alert: Alert) {
        let _ = self.network_alert_notifier.send(alert);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    pub new_block_notify_script: Option<String>,
    pub network_alert_notify_script: Option<String>,
}
