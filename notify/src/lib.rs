use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

use ckb_core::service::Request;
use ckb_shared::index::BlockCategory;
use crossbeam_channel::{select, Receiver, Sender};
use fnv::FnvHashMap;
use log::{debug, trace, warn};

pub const REGISTER_CHANNEL_SIZE: usize = 2;
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

type StopSignal = ();
pub type MsgNewTransaction = ();
pub type MsgNewBlock = Arc<BlockCategory>;
pub type NotifyRegister<M> = Sender<Request<(String, usize), Receiver<M>>>;

#[derive(Default)]
pub struct NotifyService {}

#[derive(Clone)]
pub struct NotifyController {
    signal: Sender<StopSignal>,
    new_transaction_register: NotifyRegister<MsgNewTransaction>,
    new_block_register: NotifyRegister<MsgNewBlock>,
    new_transaction_notifier: Sender<MsgNewTransaction>,
    new_block_notifier: Sender<MsgNewBlock>,
}

impl NotifyService {
    pub fn start<S: ToString>(self, thread_name: Option<S>) -> (JoinHandle<()>, NotifyController) {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(REGISTER_CHANNEL_SIZE);

        let (new_transaction_register, new_transaction_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_block_register, new_block_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);

        let (new_transaction_sender, new_transaction_receiver) =
            crossbeam_channel::bounded::<MsgNewTransaction>(NOTIFY_CHANNEL_SIZE);
        let (new_block_sender, new_block_receiver) =
            crossbeam_channel::bounded::<MsgNewBlock>(NOTIFY_CHANNEL_SIZE);

        let mut new_transaction_subscribers = FnvHashMap::default();
        let mut new_block_subscribers = FnvHashMap::default();

        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let join_handle = thread_builder
            .spawn(move || loop {
                select! {
                    recv(signal_receiver) -> _ => {
                        break;
                    }

                    recv(new_transaction_register_receiver) -> msg => Self::handle_register_new_transaction(
                        &mut new_transaction_subscribers, msg
                    ),
                    recv(new_block_register_receiver) -> msg => Self::handle_register_new_block(
                        &mut new_block_subscribers, msg
                    ),
                    recv(new_transaction_receiver) -> msg => Self::handle_notify_new_transaction(
                        &new_transaction_subscribers, msg
                    ),
                    recv(new_block_receiver) -> msg => Self::handle_notify_new_block(
                        &new_block_subscribers, msg
                    ),
                }
            }).expect("Start notify service failed");

        (
            join_handle,
            NotifyController {
                new_transaction_register,
                new_block_register,
                new_transaction_notifier: new_transaction_sender,
                new_block_notifier: new_block_sender,
                signal: signal_sender,
            },
        )
    }

    fn handle_register_new_transaction(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewTransaction>>,
        msg: Result<
            Request<(String, usize), Receiver<MsgNewTransaction>>,
            crossbeam_channel::RecvError,
        >,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
                debug!(target: "notify", "Register new_transaction {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgNewTransaction>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register new_transaction channel is closed"),
        }
    }

    fn handle_register_new_block(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewBlock>>,
        msg: Result<Request<(String, usize), Receiver<MsgNewBlock>>, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
                debug!(target: "notify", "Register new_block {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgNewBlock>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register new_block channel is closed"),
        }
    }

    fn handle_notify_new_transaction(
        subscribers: &FnvHashMap<String, Sender<MsgNewTransaction>>,
        msg: Result<MsgNewTransaction, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(()) => {
                trace!(target: "notify", "event new transaction {:?}", msg);
                for subscriber in subscribers.values() {
                    let _ = subscriber.send(());
                }
            }
            _ => warn!(target: "notify", "new transaction channel is closed"),
        }
    }

    fn handle_notify_new_block(
        subscribers: &FnvHashMap<String, Sender<MsgNewBlock>>,
        msg: Result<MsgNewBlock, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(msg) => {
                trace!(target: "notify", "event new block {:?}", msg);
                for subscriber in subscribers.values() {
                    let _ = subscriber.send(Arc::clone(&msg));
                }
            }
            _ => warn!(target: "notify", "new block channel is closed"),
        }
    }
}

impl NotifyController {
    pub fn stop(self) {
        let _ = self.signal.send(());
    }

    pub fn subscribe_new_transaction<S: ToString>(&self, name: &S) -> Receiver<MsgNewTransaction> {
        Request::call(&self.new_transaction_register, (name.to_string(), 128))
            .expect("Subscribe new transaction failed")
    }
    pub fn subscribe_new_block<S: ToString>(&self, name: &S) -> Receiver<MsgNewBlock> {
        Request::call(&self.new_block_register, (name.to_string(), 128))
            .expect("Subscribe new block failed")
    }
    pub fn notify_new_transaction(&self) {
        let _ = self.new_transaction_notifier.send(());
    }
    pub fn notify_new_block(&self, block_category: BlockCategory) {
        let _ = self.new_block_notifier.send(Arc::new(block_category));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_transaction() {
        let (handle, notify) = NotifyService::default().start::<&str>(None);
        let receiver1 = notify.subscribe_new_transaction(&"miner1");
        let receiver2 = notify.subscribe_new_transaction(&"miner2");
        notify.notify_new_transaction();
        assert_eq!(receiver1.recv(), Ok(()));
        assert_eq!(receiver2.recv(), Ok(()));
        notify.stop();
        handle.join().expect("join failed");
    }
}
