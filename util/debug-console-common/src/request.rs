use tokio::sync::oneshot;

use crate::{Command, Response};

#[derive(Debug)]
pub struct Request {
    pub cmd: Command,
    pub sender: oneshot::Sender<Response>,
}

impl Request {
    pub fn build(cmd: Command) -> (Self, oneshot::Receiver<Response>) {
        let (sender, receiver) = oneshot::channel();
        let request = Self { cmd, sender };
        (request, receiver)
    }

    pub fn cmd(&self) -> &Command {
        &self.cmd
    }

    pub fn reply(self, response: Response) -> Result<(), Response> {
        self.sender.send(response)
    }
}
