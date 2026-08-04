use super::*;

impl AuthorityRelayReceiver {
    pub(in crate::authority) fn observation(&self) -> (usize, usize) {
        let state = self.inner.state.lock();
        (state.queue.len(), state.bytes)
    }
}
