use crate::peer_store::{
    Multiaddr, PeerId, Score, SessionType, Status, ADDR_MAX_FAILURES, ADDR_MAX_RETRIES,
    ADDR_TIMEOUT_MS,
};

#[derive(Debug)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub connected_addr: Multiaddr,
    pub score: Score,
    pub status: Status,
    pub session_type: SessionType,
    pub ban_time_ms: u64,
    pub last_connected_at_ms: u64,
}

impl PeerInfo {
    pub fn new(
        peer_id: PeerId,
        connected_addr: Multiaddr,
        score: Score,
        session_type: SessionType,
        last_connected_at_ms: u64,
    ) -> Self {
        PeerInfo {
            peer_id,
            connected_addr,
            score,
            status: Status::Unknown,
            session_type,
            last_connected_at_ms,
            ban_time_ms: 0,
        }
    }
}

#[derive(Debug)]
pub struct PeerAddr {
    pub peer_id: PeerId,
    pub addr: Multiaddr,
    pub last_connected_at_ms: u64,
    pub last_tried_at_ms: u64,
    pub attempts_count: u32,
}

impl PeerAddr {
    pub fn new(peer_id: PeerId, addr: Multiaddr, last_connected_at_ms: u64) -> Self {
        PeerAddr {
            peer_id,
            addr,
            last_connected_at_ms,
            last_tried_at_ms: 0,
            attempts_count: 0,
        }
    }

    pub fn tried_in_last_minute(&self, now_ms: u64) -> bool {
        self.last_tried_at_ms >= now_ms.saturating_sub(60_000)
    }

    pub fn is_terrible(&self, now_ms: u64) -> bool {
        // do not remove addr tried in last minute
        if self.tried_in_last_minute(now_ms) {
            return false;
        }
        // we give up if never connect to this addr
        if self.last_connected_at_ms == 0 && self.attempts_count >= ADDR_MAX_RETRIES {
            return true;
        }
        // consider addr is terrible if failed too many times
        if now_ms.saturating_sub(self.last_connected_at_ms) > ADDR_TIMEOUT_MS
            && (self.attempts_count >= ADDR_MAX_FAILURES)
        {
            return true;
        }
        false
    }

    pub fn mark_tried(&mut self, tried_at_ms: u64) {
        self.last_tried_at_ms = tried_at_ms;
        self.attempts_count = self.attempts_count.saturating_add(1);
    }

    pub fn mark_connected(&mut self, connected_at_ms: u64) {
        self.last_connected_at_ms = connected_at_ms;
        // reset attemps
        self.attempts_count = 0;
    }
}
