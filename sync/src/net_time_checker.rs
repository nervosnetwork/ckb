use crate::utils::send_message_to;
use ckb_constant::sync::BAD_MESSAGE_BAN_TIME;
use ckb_logger::{debug, info, warn};
use ckb_network::async_trait;
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, bytes::Bytes};
use ckb_types::{packed, prelude::*};
use ckb_util::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;

pub(crate) const TOLERANT_OFFSET: u64 = 7_200_000;
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
        samples.sort_unstable();
        let mid = samples.len() >> 1;
        if samples.len() & 0x1 == 0 {
            // samples is even
            Some((samples[mid - 1] + samples[mid]) >> 1)
        } else {
            // samples is odd
            samples.get(mid).cloned()
        }
    }

    pub fn check(&self) -> Result<(), i64> {
        let network_offset = match self.median_offset() {
            Some(offset) => offset,
            None => return Ok(()),
        };
        if network_offset.unsigned_abs() > self.tolerant_offset {
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
            checker: RwLock::new(self.checker.read().to_owned()),
        }
    }
}

impl NetTimeProtocol {
    /// Init time protocol
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

#[async_trait]
impl CKBProtocolHandler for NetTimeProtocol {
    async fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    async fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        // send local time to inbound peers
        if let Some(true) = nc.get_peer(peer_index).map(|peer| peer.is_inbound()) {
            let now = ckb_systemtime::unix_time_as_millis();
            let time = packed::Time::new_builder().timestamp(now.pack()).build();
            let _status = send_message_to(nc.as_ref(), peer_index, &time);
        }
    }

    async fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        if let Some(true) = nc.get_peer(peer_index).map(|peer| peer.is_inbound()) {
            info!(
                "Received a time message from a non-outbound peer {}",
                peer_index
            );
        }

        let timestamp: u64 = match packed::TimeReader::from_slice(&data)
            .map(|time| time.timestamp().unpack())
            .ok()
        {
            Some(timestamp) => timestamp,
            None => {
                info!("Received a malformed message from peer {}", peer_index);
                nc.ban_peer(
                    peer_index,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        let now: u64 = ckb_systemtime::unix_time_as_millis();
        let offset: i64 = (i128::from(now) - i128::from(timestamp)) as i64;
        let mut net_time_checker = self.checker.write();
        debug!("New net time offset sample {}ms", offset);
        net_time_checker.add_sample(offset);
        if let Err(offset) = net_time_checker.check() {
            warn!(
                "Please check your computer's local clock ({}ms offset from network peers). Incorrect time setting may cause unexpected errors.",
                offset
            );
        }
    }
}
