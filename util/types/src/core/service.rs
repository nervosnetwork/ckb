use ckb_channel::Sender;
use std::sync::mpsc;

pub const SIGNAL_CHANNEL_SIZE: usize = 1;
pub const DEFAULT_CHANNEL_SIZE: usize = 32;

pub struct Request<A, R> {
    pub responder: mpsc::Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responder, response) = mpsc::channel();
        let _ = sender.send(Request {
            responder,
            arguments,
        });
        response.recv().ok()
    }
}
