#![allow(clippy::needless_pass_by_value)]

use ckb_core::block::Block;
use ckb_core::service::Request;
use ckb_logger::{debug, trace, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use crossbeam_channel::{select, Receiver, Sender};
use fnv::FnvHashMap;
use std::sync::Arc;
use std::thread;

pub const SIGNAL_CHANNEL_SIZE: usize = 1;
pub const REGISTER_CHANNEL_SIZE: usize = 2;
pub const NOTIFY_CHANNEL_SIZE: usize = 128;

pub type MsgNewTransaction = ();
// pub type MsgNewTip = Arc<Block>;
pub type MsgNewUncle = Arc<Block>;
// pub type MsgSwitchFork = Arc<ForkBlocks>;
pub type NotifyRegister<M> = Sender<Request<(String, usize), Receiver<M>>>;

#[derive(Default)]
pub struct NotifyService {}

#[derive(Clone)]
pub struct NotifyController {
    stop: StopHandler<()>,
    // new_transaction_register: NotifyRegister<MsgNewTransaction>,
    // new_tip_register: NotifyRegister<MsgNewTip>,
    new_uncle_register: NotifyRegister<MsgNewUncle>,
    // switch_fork_register: NotifyRegister<MsgSwitchFork>,
    // new_transaction_notifier: Sender<MsgNewTransaction>,
    // new_tip_notifier: Sender<MsgNewTip>,
    new_uncle_notifier: Sender<MsgNewUncle>,
    // switch_fork_notifier: Sender<MsgSwitchFork>,
}

impl Drop for NotifyController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl NotifyService {
    pub fn start<S: ToString>(self, thread_name: Option<S>) -> NotifyController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        // let (new_transaction_register, new_transaction_register_receiver) =
        //     crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        // let (new_tip_register, new_tip_register_receiver) =
        //     crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        let (new_uncle_register, new_uncle_register_receiver) =
            crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);
        // let (switch_fork_register, switch_fork_register_receiver) =
        //     crossbeam_channel::bounded(REGISTER_CHANNEL_SIZE);

        // let (new_transaction_sender, new_transaction_receiver) =
        //     crossbeam_channel::bounded::<MsgNewTransaction>(NOTIFY_CHANNEL_SIZE);
        // let (new_tip_sender, new_tip_receiver) =
        //     crossbeam_channel::bounded::<MsgNewTip>(NOTIFY_CHANNEL_SIZE);
        let (new_uncle_sender, new_uncle_receiver) =
            crossbeam_channel::bounded::<MsgNewUncle>(NOTIFY_CHANNEL_SIZE);
        // let (switch_fork_sender, switch_fork_receiver) =
        //     crossbeam_channel::bounded::<MsgSwitchFork>(NOTIFY_CHANNEL_SIZE);

        // let mut new_transaction_subscribers = FnvHashMap::default();
        // let mut new_tip_subscribers = FnvHashMap::default();
        let mut new_uncle_subscribers = FnvHashMap::default();
        // let mut switch_fork_subscribers = FnvHashMap::default();

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

                    // recv(new_transaction_register_receiver) -> msg => Self::handle_register_new_transaction(
                    //     &mut new_transaction_subscribers, msg
                    // ),
                    // recv(new_tip_register_receiver) -> msg => Self::handle_register_new_tip(
                    //     &mut new_tip_subscribers, msg
                    // ),
                    recv(new_uncle_register_receiver) -> msg => Self::handle_register_new_uncle(
                        &mut new_uncle_subscribers, msg
                    ),
                    // recv(switch_fork_register_receiver) -> msg => Self::handle_register_switch_fork(
                    //     &mut switch_fork_subscribers, msg
                    // ),

                    // recv(new_transaction_receiver) -> msg => Self::handle_notify_new_transaction(
                    //     &new_transaction_subscribers, msg
                    // ),
                    // recv(new_tip_receiver) -> msg => Self::handle_notify_new_tip(
                    //     &new_tip_subscribers, msg
                    // ),
                    recv(new_uncle_receiver) -> msg => Self::handle_notify_new_uncle(
                        &new_uncle_subscribers, msg
                    ),
                    // recv(switch_fork_receiver) -> msg => Self::handle_notify_switch_fork(
                    //     &switch_fork_subscribers, msg
                    // )
                }
            })
            .expect("Start notify service failed");

        NotifyController {
            // new_transaction_register,
            // new_tip_register,
            new_uncle_register,
            // switch_fork_register,
            // new_transaction_notifier: new_transaction_sender,
            // new_tip_notifier: new_tip_sender,
            new_uncle_notifier: new_uncle_sender,
            // switch_fork_notifier: switch_fork_sender,
            stop: StopHandler::new(SignalSender::Crossbeam(signal_sender), join_handle),
        }
    }

    // fn handle_register_new_transaction(
    //     subscribers: &mut FnvHashMap<String, Sender<MsgNewTransaction>>,
    //     msg: Result<
    //         Request<(String, usize), Receiver<MsgNewTransaction>>,
    //         crossbeam_channel::RecvError,
    //     >,
    // ) {
    //     match msg {
    //         Ok(Request {
    //             responder,
    //             arguments: (name, capacity),
    //         }) => {
    //             debug!("Register new_transaction {:?}", name);
    //             let (sender, receiver) = crossbeam_channel::bounded::<MsgNewTransaction>(capacity);
    //             subscribers.insert(name, sender);
    //             let _ = responder.send(receiver);
    //         }
    //         _ => warn!("Register new_transaction channel is closed"),
    //     }
    // }

    // fn handle_register_new_tip(
    //     subscribers: &mut FnvHashMap<String, Sender<MsgNewTip>>,
    //     msg: Result<Request<(String, usize), Receiver<MsgNewTip>>, crossbeam_channel::RecvError>,
    // ) {
    //     match msg {
    //         Ok(Request {
    //             responder,
    //             arguments: (name, capacity),
    //         }) => {
    //             debug!("Register new_tip {:?}", name);
    //             let (sender, receiver) = crossbeam_channel::bounded::<MsgNewTip>(capacity);
    //             subscribers.insert(name, sender);
    //             let _ = responder.send(receiver);
    //         }
    //         _ => warn!("Register new_tip channel is closed"),
    //     }
    // }

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

    // fn handle_register_switch_fork(
    //     subscribers: &mut FnvHashMap<String, Sender<MsgSwitchFork>>,
    //     msg: Result<
    //         Request<(String, usize), Receiver<MsgSwitchFork>>,
    //         crossbeam_channel::RecvError,
    //     >,
    // ) {
    //     match msg {
    //         Ok(Request {
    //             responder,
    //             arguments: (name, capacity),
    //         }) => {
    //             debug!("Register switch_fork {:?}", name);
    //             let (sender, receiver) = crossbeam_channel::bounded::<MsgSwitchFork>(capacity);
    //             subscribers.insert(name, sender);
    //             let _ = responder.send(receiver);
    //         }
    //         _ => warn!("Register switch_fork channel is closed"),
    //     }
    // }

    // fn handle_notify_new_transaction(
    //     subscribers: &FnvHashMap<String, Sender<MsgNewTransaction>>,
    //     msg: Result<MsgNewTransaction, crossbeam_channel::RecvError>,
    // ) {
    //     match msg {
    //         Ok(()) => {
    //             trace!("event new transaction {:?}", msg);
    //             for subscriber in subscribers.values() {
    //                 let _ = subscriber.send(());
    //             }
    //         }
    //         _ => warn!("new transaction channel is closed"),
    //     }
    // }

    // fn handle_notify_new_tip(
    //     subscribers: &FnvHashMap<String, Sender<MsgNewTip>>,
    //     msg: Result<MsgNewTip, crossbeam_channel::RecvError>,
    // ) {
    //     match msg {
    //         Ok(msg) => {
    //             trace!("event new tip {:?}", msg);
    //             for subscriber in subscribers.values() {
    //                 let _ = subscriber.send(Arc::clone(&msg));
    //             }
    //         }
    //         _ => warn!("new tip channel is closed"),
    //     }
    // }

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

    // fn handle_notify_switch_fork(
    //     subscribers: &FnvHashMap<String, Sender<MsgSwitchFork>>,
    //     msg: Result<MsgSwitchFork, crossbeam_channel::RecvError>,
    // ) {
    //     match msg {
    //         Ok(msg) => {
    //             trace!("event switch fork {:?}", msg);
    //             for subscriber in subscribers.values() {
    //                 let _ = subscriber.send(Arc::clone(&msg));
    //             }
    //         }
    //         _ => warn!("event 3 channel is closed"),
    //     }
    // }
}

impl NotifyController {
    // pub fn subscribe_new_transaction<S: ToString>(&self, name: S) -> Receiver<MsgNewTransaction> {
    //     Request::call(&self.new_transaction_register, (name.to_string(), 128))
    //         .expect("Subscribe new transaction failed")
    // }
    // pub fn subscribe_new_tip<S: ToString>(&self, name: S) -> Receiver<MsgNewTip> {
    //     Request::call(&self.new_tip_register, (name.to_string(), 128))
    //         .expect("Subscribe new tip failed")
    // }
    pub fn subscribe_new_uncle<S: ToString>(&self, name: S) -> Receiver<MsgNewUncle> {
        Request::call(&self.new_uncle_register, (name.to_string(), 128))
            .expect("Subscribe new uncle failed")
    }
    // pub fn subscribe_switch_fork<S: ToString>(&self, name: S) -> Receiver<MsgSwitchFork> {
    //     Request::call(&self.switch_fork_register, (name.to_string(), 128))
    //         .expect("Subscribe switch fork failed")
    // }

    // pub fn notify_new_transaction(&self) {
    //     let _ = self.new_transaction_notifier.send(());
    // }
    // pub fn notify_new_tip(&self, block: MsgNewTip) {
    //     let _ = self.new_tip_notifier.send(block);
    // }
    pub fn notify_new_uncle(&self, block: MsgNewUncle) {
        let _ = self.new_uncle_notifier.send(block);
    }
    // pub fn notify_switch_fork(&self, txs: MsgSwitchFork) {
    //     let _ = self.switch_fork_notifier.send(txs);
    // }
}

#[cfg(test)]
mod tests {
    // use super::*;

    // #[test]
    // fn test_new_transaction() {
    //     let notify = NotifyService::default().start::<&str>(None);
    //     let receiver1 = notify.subscribe_new_transaction("miner1");
    //     let receiver2 = notify.subscribe_new_transaction("miner2");
    //     notify.notify_new_transaction();
    //     assert_eq!(receiver1.recv(), Ok(()));
    //     assert_eq!(receiver2.recv(), Ok(()));
    // }

    // #[test]
    // fn test_new_tip() {
    //     let tip = Arc::new(Block::default());
    //     let notify = NotifyService::default().start::<&str>(None);
    //     let receiver1 = notify.subscribe_new_tip("miner1");
    //     let receiver2 = notify.subscribe_new_tip("miner2");
    //     notify.notify_new_tip(Arc::clone(&tip));
    //     assert_eq!(receiver1.recv(), Ok(Arc::clone(&tip)));
    //     assert_eq!(receiver2.recv(), Ok(tip));
    // }

    // #[test]
    // fn test_switch_fork() {
    //     let blks = Arc::new(ForkBlocks::default());
    //     let notify = NotifyService::default().start::<&str>(None);
    //     let receiver1 = notify.subscribe_switch_fork("miner1");
    //     let receiver2 = notify.subscribe_switch_fork("miner2");
    //     notify.notify_switch_fork(Arc::clone(&blks));
    //     assert_eq!(receiver1.recv(), Ok(Arc::clone(&blks)));
    //     assert_eq!(receiver2.recv(), Ok(blks));
    // }
}
