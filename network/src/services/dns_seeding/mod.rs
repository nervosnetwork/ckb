use std::{error::Error, sync::Arc, time::Duration};

use ckb_logger::{debug, error, trace, warn};
use faster_hex::hex_decode;
use secp256k1::key::PublicKey;
use tokio::time::Interval;

mod seed_record;

#[cfg(test)]
mod tests;

use crate::NetworkState;
use seed_record::SeedRecord;

// FIXME: should replace this later
const TXT_VERIFY_PUBKEY: &str = "";

pub(crate) struct DnsSeedingService {
    network_state: Arc<NetworkState>,
    check_interval: Interval,
    seeds: Vec<String>,
}

impl DnsSeedingService {
    pub(crate) fn new(network_state: Arc<NetworkState>, seeds: Vec<String>) -> DnsSeedingService {
        let check_interval = tokio::time::interval(Duration::from_secs(10));
        DnsSeedingService {
            network_state,
            check_interval,
            seeds,
        }
    }

    pub(crate) async fn start(mut self) {
        loop {
            self.check_interval.tick().await;
            if let Err(err) = self.seeding().await {
                error!("seeding error: {:?}", err);
            }
        }
    }

    async fn seeding(&self) -> Result<(), Box<dyn Error>> {
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

        let resolver = trust_dns_resolver::AsyncResolver::tokio_from_system_conf()
            .map_err(|err| format!("Failed to create DNS resolver: {}", err))?;

        let mut addrs = Vec::new();
        for seed in &self.seeds {
            debug!("query txt records from: {}", seed);
            match resolver.txt_lookup(seed.as_str()).await {
                Ok(records) => {
                    for record in records.iter() {
                        for inner in record.iter() {
                            match std::str::from_utf8(inner) {
                                Ok(record) => {
                                    match SeedRecord::decode_with_pubkey(record, &pubkey) {
                                        Ok(seed_record) => {
                                            let address = seed_record.address();
                                            trace!("got dns txt address: {}", address);
                                            addrs.push(address);
                                        }
                                        Err(err) => {
                                            debug!(
                                                "decode dns txt record failed: {:?}, {:?}",
                                                err, record
                                            );
                                        }
                                    }
                                }
                                Err(err) => {
                                    debug!("get dns txt record error: {:?}", err);
                                }
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
            for addr in addrs {
                let _ = peer_store.add_addr(addr, 0x0);
            }
        });
        Ok(())
    }
}
