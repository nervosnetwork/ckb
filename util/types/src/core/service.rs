//! Types for CKB services.
//!
//! A CKB service acts as an actor, which processes requests from a channel and sends back the
//! response via one shot channel.
use ckb_channel::Sender;
use std::sync::mpsc;

/// Default channel size to send control signals.
pub const SIGNAL_CHANNEL_SIZE: usize = 1;
/// Default channel size to send messages.
pub const DEFAULT_CHANNEL_SIZE: usize = 32;

/// Synchronous request sent to the service.
pub struct Request<A, R> {
    /// One shot channel for the service to send back the response.
    pub responder: mpsc::Sender<R>,
    /// Request arguments.
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    /// Call the service with the arguments and wait for the response.
    pub fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responder, response) = mpsc::channel();
        let _ = sender.send(Request {
            responder,
            arguments,
        });
        response.recv().ok()
    }
}
