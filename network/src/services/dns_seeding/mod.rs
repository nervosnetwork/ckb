use std::{
    error::Error,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use ckb_logger::{debug, error, info, trace, warn};
use faster_hex::hex_decode;
use futures::{Future, Stream};
use p2p::{multiaddr::Protocol, secio::PeerId};
use resolve::record::Txt;
use resolve::{DnsConfig, DnsResolver};
use secp256k1::key::PublicKey;
use tokio::time::Interval;

mod seed_record;

use crate::NetworkState;
use seed_record::SeedRecord;

// FIXME: should replace this later
const TXT_VERIFY_PUBKEY: &str = "";

pub(crate) struct DnsSeedingService {
    network_state: Arc<NetworkState>,
    wait_until: Instant,
    // Because tokio timer is not reliable
    check_interval: Interval,
    seeds: Vec<String>,
}

impl DnsSeedingService {
    pub(crate) fn new(network_state: Arc<NetworkState>, seeds: Vec<String>) -> DnsSeedingService {
        let wait_until = if network_state
            .with_peer_store_mut(|peer_store| peer_store.fetch_random_addrs(1).is_empty())
        {
            info!("No peer in peer store, start seeding...");
            Instant::now()
        } else {
            Instant::now() + Duration::from_secs(11)
        };
        let check_interval = tokio::time::interval(Duration::from_secs(1));
        DnsSeedingService {
            network_state,
            wait_until,
            check_interval,
            seeds,
        }
    }

    fn seeding(&self) -> Result<(), Box<dyn Error>> {
        // TODO: DNS seeding is disabled now, may enable in the future (need discussed)
        if TXT_VERIFY_PUBKEY.is_empty() {
            return Ok(());
        }

        let enough_outbound = self.network_state.with_peer_registry(|reg| {
            reg.peers()
                .values()
                .filter(|peer| peer.is_outbound())
                .count()
                >= 2
        });
        if enough_outbound {
            debug!("Enough outbound peers");
            return Ok(());
        }

        let mut pubkey_bytes = [4u8; 65];
        hex_decode(TXT_VERIFY_PUBKEY.as_bytes(), &mut pubkey_bytes[1..65])
            .map_err(|err| format!("parse key({}) error: {:?}", TXT_VERIFY_PUBKEY, err))?;
        let pubkey = PublicKey::from_slice(&pubkey_bytes)
            .map_err(|err| format!("create PublicKey failed: {:?}", err))?;

        let resolver = DnsConfig::load_default()
            .map_err(|err| format!("Failed to load system configuration: {}", err))
            .and_then(|config| {
                DnsResolver::new(config)
                    .map_err(|err| format!("Failed to create DNS resolver: {}", err))
            })?;

        let mut addrs = Vec::new();
        for seed in &self.seeds {
            debug!("query txt records from: {}", seed);
            match resolver.resolve_record::<Txt>(seed) {
                Ok(records) => {
                    for record in records {
                        match std::str::from_utf8(&record.data) {
                            Ok(record) => match SeedRecord::decode_with_pubkey(&record, &pubkey) {
                                Ok(seed_record) => {
                                    let address = seed_record.address();
                                    trace!("got dns txt address: {}", address);
                                    addrs.push(address);
                                }
                                Err(err) => {
                                    debug!("decode dns txt record failed: {:?}, {:?}", err, record);
                                }
                            },
                            Err(err) => {
                                debug!("get dns txt record error: {:?}", err);
                            }
                        }
                    }
                }
                Err(_) => {
                    warn!("Invalid domain name: {}", seed);
                }
            }
        }

        debug!("DNS seeding got {} address", addrs.len());
        self.network_state.with_peer_store_mut(|peer_store| {
            for mut addr in addrs {
                match addr.pop() {
                    Some(Protocol::P2P(key)) => {
                        if let Ok(peer_id) = PeerId::from_bytes(key.to_vec()) {
                            if let Err(err) = peer_store.add_addr(peer_id.clone(), addr) {
                                debug!(
                                    "failed to add addrs to peer_store: {:?}, {:?}",
                                    err, peer_id
                                );
                            }
                        }
                    }
                    _ => {
                        debug!("Got addr without peer_id: {}, ignore it", addr);
                    }
                }
            }
        });
        Ok(())
    }
}

impl Future for DnsSeedingService {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match Pin::new(&mut self.check_interval).as_mut().poll_next(cx) {
                Poll::Ready(Some(_)) => {
                    if self.wait_until < Instant::now() {
                        if let Err(err) = self.seeding() {
                            error!("seeding error: {:?}", err);
                        }
                        debug!("DNS seeding finished");
                        return Poll::Ready(());
                    } else {
                        trace!("DNS check interval");
                    }
                }
                Poll::Ready(None) => {
                    warn!("Poll DnsSeedingService interval return None");
                    return Poll::Ready(());
                }
                Poll::Pending => break,
            }
        }
        Poll::Pending
    }
}
