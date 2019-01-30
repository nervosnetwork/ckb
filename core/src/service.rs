use crossbeam_channel::{self, Sender};

const ONESHOT_CHANNEL_SIZE: usize = 1;
pub const SIGNAL_CHANNEL_SIZE: usize = 1;
pub const DEFAULT_CHANNEL_SIZE: usize = 32;

pub struct Request<A, R> {
    pub responder: Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responder, response) = crossbeam_channel::bounded(ONESHOT_CHANNEL_SIZE);
        let _ = sender.send(Request {
            responder,
            arguments,
        });
        response.recv().ok()
    }
}
