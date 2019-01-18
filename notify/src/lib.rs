use ckb_core::header::BlockNumber;
use numext_fixed_hash::H256;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

use ckb_core::block::Block;
use ckb_core::service::Request;
use crossbeam_channel::{select, Receiver, Sender};
use fnv::FnvHashMap;
use log::{debug, trace, warn};

pub const REGISTER_CHANNEL_SIZE: usize = 2;
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

#[derive(Debug, Clone)]
pub struct Forks {
    /// Ancestor block's number in main branch
    pub ancestor: BlockNumber,
    /// Side branch block hashes, from ancestor to side branch tip
    pub side_blocks: Vec<H256>,
    /// Main branch block hashes, from ancestor to main branch tip
    pub main_blocks: Vec<H256>,
}

#[derive(Debug, Clone)]
pub enum BlockCategory {
    MainBranch(H256),
    SideBranch(H256),
    SideSwitchToMain(Forks),
}

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
<<<<<<< HEAD
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_tip_register, new_tip_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_uncle_register, new_uncle_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (switch_fork_register, switch_fork_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);

        let (new_transaction_sender, new_transaction_receiver) =
            crossbeam_channel::bounded::<MsgNewTransaction>(NOTIFY_CHANNEL_SIZE);
        let (new_tip_sender, new_tip_receiver) =
            crossbeam_channel::bounded::<MsgNewTip>(NOTIFY_CHANNEL_SIZE);
        let (new_uncle_sender, new_uncle_receiver) =
            crossbeam_channel::bounded::<MsgNewUncle>(NOTIFY_CHANNEL_SIZE);
        let (switch_fork_sender, switch_fork_receiver) =
            crossbeam_channel::bounded::<MsgSwitchFork>(NOTIFY_CHANNEL_SIZE);
=======
            channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_block_register, new_block_register_receiver) =
            channel::bounded(REGISTER_CHANNEL_SIZE);

        let (new_transaction_sender, new_transaction_receiver) =
            channel::bounded::<MsgNewTransaction>(NOTIFY_CHANNEL_SIZE);
        let (new_block_sender, new_block_receiver) =
            channel::bounded::<MsgNewBlock>(NOTIFY_CHANNEL_SIZE);
>>>>>>> refactor: Notify and ChainIndex update

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

<<<<<<< HEAD
    fn handle_register_new_tip(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewTip>>,
        msg: Result<Request<(String, usize), Receiver<MsgNewTip>>, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
                debug!(target: "notify", "Register new_tip {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgNewTip>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register new_tip channel is closed"),
        }
    }

    fn handle_register_new_uncle(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewUncle>>,
        msg: Result<Request<(String, usize), Receiver<MsgNewUncle>>, crossbeam_channel::RecvError>,
=======
    fn handle_register_new_block(
        subscribers: &mut FnvHashMap<String, Sender<MsgNewBlock>>,
        msg: Result<Request<(String, usize), Receiver<MsgNewBlock>>, channel::RecvError>,
>>>>>>> refactor: Notify and ChainIndex update
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
<<<<<<< HEAD
                debug!(target: "notify", "Register new_uncle {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgNewUncle>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register new_uncle channel is closed"),
        }
    }

    fn handle_register_switch_fork(
        subscribers: &mut FnvHashMap<String, Sender<MsgSwitchFork>>,
        msg: Result<
            Request<(String, usize), Receiver<MsgSwitchFork>>,
            crossbeam_channel::RecvError,
        >,
    ) {
        match msg {
            Ok(Request {
                responder,
                arguments: (name, capacity),
            }) => {
                debug!(target: "notify", "Register switch_fork {:?}", name);
                let (sender, receiver) = crossbeam_channel::bounded::<MsgSwitchFork>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register switch_fork channel is closed"),
=======
                debug!(target: "notify", "Register new_block {:?}", name);
                let (sender, receiver) = channel::bounded::<MsgNewBlock>(capacity);
                subscribers.insert(name, sender);
                let _ = responder.send(receiver);
            }
            _ => warn!(target: "notify", "Register new_block channel is closed"),
>>>>>>> refactor: Notify and ChainIndex update
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

<<<<<<< HEAD
    fn handle_notify_new_tip(
        subscribers: &FnvHashMap<String, Sender<MsgNewTip>>,
        msg: Result<MsgNewTip, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(msg) => {
                trace!(target: "notify", "event new tip {:?}", msg);
                for subscriber in subscribers.values() {
                    let _ = subscriber.send(Arc::clone(&msg));
                }
            }
            _ => warn!(target: "notify", "new tip channel is closed"),
        }
    }

    fn handle_notify_new_uncle(
        subscribers: &FnvHashMap<String, Sender<MsgNewUncle>>,
        msg: Result<MsgNewUncle, crossbeam_channel::RecvError>,
    ) {
        match msg {
            Ok(msg) => {
                trace!(target: "notify", "event new uncle {:?}", msg);
                for subscriber in subscribers.values() {
                    let _ = subscriber.send(Arc::clone(&msg));
                }
            }
            _ => warn!(target: "notify", "new uncle channel is closed"),
        }
    }

    fn handle_notify_switch_fork(
        subscribers: &FnvHashMap<String, Sender<MsgSwitchFork>>,
        msg: Result<MsgSwitchFork, crossbeam_channel::RecvError>,
=======
    fn handle_notify_new_block(
        subscribers: &FnvHashMap<String, Sender<MsgNewBlock>>,
        msg: Result<MsgNewBlock, channel::RecvError>,
>>>>>>> refactor: Notify and ChainIndex update
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
