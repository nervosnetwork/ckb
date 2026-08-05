use super::*;

impl AuthorityRelayReceiver {
    pub(in crate::authority) fn observation(&self) -> (usize, usize) {
        let state = self.inner.state.lock();
        (state.queue.len(), state.bytes)
    }

    pub(in crate::authority) fn corrupt_bytes_for_test(&self, bytes: usize) {
        self.inner.state.lock().bytes = bytes;
    }
}
