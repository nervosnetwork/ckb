use crate::BAD_MESSAGE_BAN_TIME;
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_protocol::{get_root, TimeMessage};
use ckb_util::RwLock;
use flatbuffers::FlatBufferBuilder;
use log::{debug, info, warn};
use std::collections::VecDeque;

const TOLERANT_OFFSET: u64 = 7_200_000;
const MIN_SAMPLES: usize = 5;
const MAX_SAMPLES: usize = 11;

/// Collect and check time offset samples
#[derive(Clone)]
pub struct NetTimeChecker {
    /// Local clock should has less offset than this value.
    tolerant_offset: u64,
    max_samples: usize,
    min_samples: usize,
    samples: VecDeque<i64>,
}

impl NetTimeChecker {
    pub fn new(min_samples: usize, max_samples: usize, tolerant_offset: u64) -> Self {
        NetTimeChecker {
            min_samples,
            max_samples,
            tolerant_offset,
            samples: VecDeque::with_capacity(max_samples + 1),
        }
    }

    pub fn add_sample(&mut self, offset: i64) {
        self.samples.push_back(offset);
        if self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
    }

    fn median_offset(&self) -> Option<i64> {
        if self.samples.is_empty() || self.samples.len() < self.min_samples {
            return None;
        }
        let mut samples = self.samples.iter().cloned().collect::<Vec<_>>();
        samples.sort();
        if samples.len() % 2 == 0 {
            // samples is even
            let i = samples.len() / 2;
            Some((samples[i - 1] + samples[i]) / 2)
        } else {
            // samples is odd
            samples.get(samples.len() / 2).cloned()
        }
    }

    pub fn check(&self) -> Result<(), i64> {
        let network_offset = match self.median_offset() {
            Some(offset) => offset,
            None => return Ok(()),
        };
        if network_offset.abs() as u64 > self.tolerant_offset {
            return Err(network_offset);
        }
        Ok(())
    }
}

impl Default for NetTimeChecker {
    fn default() -> Self {
        NetTimeChecker::new(MIN_SAMPLES, MAX_SAMPLES, TOLERANT_OFFSET)
    }
}

/// Collect time offset samples from network peers and send notify to user if offset is too large
pub struct NetTimeProtocol {
    checker: RwLock<NetTimeChecker>,
}

impl Clone for NetTimeProtocol {
    fn clone(&self) -> Self {
        NetTimeProtocol {
            checker: RwLock::new(self.checker.read().clone()),
        }
    }
}

impl NetTimeProtocol {
    pub fn new(min_samples: usize, max_samples: usize, tolerant_offset: u64) -> Self {
        let checker = RwLock::new(NetTimeChecker::new(
            min_samples,
            max_samples,
            tolerant_offset,
        ));
        NetTimeProtocol { checker }
    }
}

impl Default for NetTimeProtocol {
    fn default() -> Self {
        let checker = RwLock::new(NetTimeChecker::default());
        NetTimeProtocol { checker }
    }
}

impl CKBProtocolHandler for NetTimeProtocol {
    fn init(&mut self, _nc: Box<dyn CKBProtocolContext>) {}

    fn connected(
        &mut self,
        nc: Box<dyn CKBProtocolContext>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        // send local time to inbound peers
        if let Some(true) = nc.get_peer(peer_index).map(|peer| peer.is_inbound()) {
            let now = faketime::unix_time_as_millis();
            let fbb = &mut FlatBufferBuilder::new();
            let message = TimeMessage::build_time(fbb, now);
            fbb.finish(message, None);
            nc.send_message_to(peer_index, fbb.finished_data().to_vec());
        }
    }

    fn received(
        &mut self,
        nc: Box<dyn CKBProtocolContext>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        if let Some(true) = nc.get_peer(peer_index).map(|peer| peer.is_inbound()) {
            info!(target: "network", "Peer {} is not outbound but sends us time message", peer_index);
        }

        let timestamp = match get_root::<TimeMessage>(&data)
            .ok()
            .and_then(|m| m.payload())
            .map(|p| p.timestamp())
        {
            Some(timestamp) => timestamp,
            None => {
                info!(target: "network", "Peer {} sends us malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };

        let now: u64 = faketime::unix_time_as_millis();
        let offset: i64 = (i128::from(now) - i128::from(timestamp)) as i64;
        let mut net_time_checker = self.checker.write();
        debug!(target: "network", "new net time offset sample {}ms", offset);
        net_time_checker.add_sample(offset);
        if let Err(offset) = net_time_checker.check() {
            warn!(target: "network", "Please check your computer's local clock({}ms offset from network peers), If your clock is wrong, it may cause unexpected errors.", offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_samples_collect() {
        let mut ntc = NetTimeChecker::new(3, 5, TOLERANT_OFFSET);
        // zero samples
        assert!(ntc.check().is_ok());
        // 1 sample
        ntc.add_sample(TOLERANT_OFFSET as i64 + 1);
        assert!(ntc.check().is_ok());
        // 3 samples
        ntc.add_sample(TOLERANT_OFFSET as i64 + 2);
        ntc.add_sample(TOLERANT_OFFSET as i64 + 3);
        assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 2);
        // 4 samples
        ntc.add_sample(1);
        assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
        // 5 samples
        ntc.add_sample(2);
        assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
        // 5 samples within tolerant offset
        ntc.add_sample(3);
        ntc.add_sample(4);
        ntc.add_sample(5);
        assert!(ntc.check().is_ok());
        // 5 samples negative offset
        ntc.add_sample(-(TOLERANT_OFFSET as i64) - 1);
        ntc.add_sample(-(TOLERANT_OFFSET as i64) - 2);
        assert!(ntc.check().is_ok());
        ntc.add_sample(-(TOLERANT_OFFSET as i64) - 3);
        assert_eq!(ntc.check().unwrap_err(), -(TOLERANT_OFFSET as i64) - 1);
    }
}
