struct Feeler {}

impl CKBProtocolHandler for Feeler {
    fn initialize(&self, nc: Box<CKBProtocolContext>) {
        let _ = nc.register_timer(TX_PROPOSAL_TOKEN, Duration::from_millis(100));
    }

    fn received(&self, nc: Box<CKBProtocolContext>, peer: PeerIndex, data: Bytes) {
        // TODO use flatbuffers verifier
        let msg = get_root::<RelayMessage>(&data);
        debug!(target: "relay", "msg {:?}", msg.payload_type());
        self.process(nc.as_ref(), peer, msg);
    }

    fn connected(&self, nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        info!(target: "feeler", "peer={} FeelerProtocol.connected", peer);
    }

    fn disconnected(&self, _nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        info!(target: "relay", "peer={} RelayProtocol.disconnected", peer);
        // TODO
    }

    fn timer_triggered(&self, nc: Box<CKBProtocolContext>, token: TimerToken) {
        match token as usize {
            TX_PROPOSAL_TOKEN => self.prune_tx_proposal_request(nc.as_ref()),
            _ => unreachable!(),
        }
    }
}
