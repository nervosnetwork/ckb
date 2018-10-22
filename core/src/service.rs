use channel::{self, Sender};

const ONESHOT_CHANNEL_SIZE: usize = 1;
pub const DEFAULT_CHANNEL_SIZE: usize = 32;

pub struct Request<A, R> {
    pub responsor: Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responsor, response) = channel::bounded(ONESHOT_CHANNEL_SIZE);
        sender.send(Request {
            responsor,
            arguments,
        });
        response.recv()
    }
}
