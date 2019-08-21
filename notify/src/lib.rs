#![allow(clippy::needless_pass_by_value)]

use ckb_logger::{debug, trace, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{core::service::Request, packed::UncleBlock};
use crossbeam_channel::{select, Receiver, Sender};
use fnv::FnvHashMap;
use std::sync::Arc;
use std::thread;

pub const SIGNAL_CHANNEL_SIZE: usize = 1;
pub const REGISTER_CHANNEL_SIZE: usize = 2;
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

pub type MsgNewTransaction = ();
pub type MsgNewUncle = Arc<UncleBlock>;
pub type NotifyRegister<M> = Sender<Request<(String, usize), Receiver<M>>>;

#[derive(Default)]
pub struct NotifyService {}

#[derive(Clone)]
pub struct NotifyController {
    stop: StopHandler<()>,
    new_uncle_register: NotifyRegister<MsgNewUncle>,
    new_uncle_notifier: Sender<MsgNewUncle>,
}

impl Drop for NotifyController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl NotifyService {
    // remove `allow` tag when https://github.com/crossbeam-rs/crossbeam/issues/404 is solved
    #[allow(clippy::zero_ptr, clippy::drop_copy)]
    pub fn start<S: ToString>(self, thread_name: Option<S>) -> NotifyController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (new_uncle_register, new_uncle_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_uncle_sender, new_uncle_receiver) =
            crossbeam_channel::bounded::<MsgNewUncle>(NOTIFY_CHANNEL_SIZE);
        let mut new_uncle_subscribers = FnvHashMap::default();

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
                    recv(new_uncle_register_receiver) -> msg => Self::handle_register_new_uncle(
                        &mut new_uncle_subscribers, msg
                    ),
                    recv(new_uncle_receiver) -> msg => Self::handle_notify_new_uncle(
                        &new_uncle_subscribers, msg
                    ),
                }
            })
            .expect("Start notify service failed");

        NotifyController {
            new_uncle_register,
            new_uncle_notifier: new_uncle_sender,
            stop: StopHandler::new(SignalSender::Crossbeam(signal_sender), join_handle),
        }
    }

    fn handle_register_new_uncle(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewUncle>>,
        msg: Result<Request<(String, usize), Receiver<MsgNewUncle>>, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
                debug!("Register new_uncle {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgNewUncle>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!("Register new_uncle channel is closed"),
        }
    }

    fn handle_notify_new_uncle(
        subscribers: &FnvHashMap<String, Sender<MsgNewUncle>>,
        msg: Result<MsgNewUncle, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(msg) => {
                trace!("event new uncle {:?}", msg);
                for subscriber in subscribers.values() {
                    let _ = subscriber.send(Arc::clone(&msg));
                }
            }
            _ => warn!("new uncle channel is closed"),
        }
    }
}

impl NotifyController {
    pub fn subscribe_new_uncle<S: ToString>(&self, name: S) -> Receiver<MsgNewUncle> {
        Request::call(&self.new_uncle_register, (name.to_string(), 128))
            .expect("Subscribe new uncle failed")
    }
    pub fn notify_new_uncle(&self, block: MsgNewUncle) {
        let _ = self.new_uncle_notifier.send(block);
    }
}
