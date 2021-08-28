use std::{
    borrow::Cow,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use ckb_hash::blake2b_256;
use lazy_static::lazy_static;
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    secio::PeerId,
    utils::{is_reachable, socketaddr_to_multiaddr},
};
use secp256k1::{
    key::PublicKey,
    recovery::{RecoverableSignature, RecoveryId},
    Message,
};

lazy_static! {
    pub(crate) static ref SECP256K1: secp256k1::Secp256k1<secp256k1::All> =
        secp256k1::Secp256k1::new();
}
pub(crate) const SEP: char = ';';

// Format:
// ======
// {ip};{port};{peer_id(base58)};{valid_until(base10)};{signature(base58)}

// Length calculation:
// ==================
// ip          : max   39 bytes (2001:0dc5:72a3:0000:0000:802e:3370:73E4)
// port        : max   5 bytes (65535)
// peer_id     : max   (32 + 3) * 2 * 0.8 = 56 bytes (base58)
// valid_until : max   11 bytes (31536000000, 1000 year)
// signature   : max   65 * 2 * 0.8 = 104 bytes (base58)
// sep         : exact 4 bytes
// total       : max   39 + 5 + 56 + 11 + 104 + 4 = 224 bytes
// txt limit   : 255 bytes (enough)

// Typical txt record:
// ==================
//   47.103.65.40;49582;QmbU82jmDbu8AsUfa6bDKPHxTpwnPfcRQrzNPacKcSyM1Y;1574942409;K1vAkHZZ8to5VmjD4eyv65ENLbNa9Tda4Aytd8DE9iipFQanRpcZtSPyRiiGHThRGJPVRD18KAsGb8kV2s2WBK39R
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SeedRecord {
    pub(crate) ip: IpAddr,
    pub(crate) port: u16,
    pub(crate) peer_id: Option<PeerId>,
    // Future utc timestamp
    pub(crate) valid_until: u64,
    pub(crate) pubkey: PublicKey,
}

impl SeedRecord {
    pub fn check(&self) -> Result<(), SeedRecordError> {
        if !is_reachable(self.ip) {
            return Err(SeedRecordError::InvalidIp(self.ip));
        }

        if self.port == 0 {
            return Err(SeedRecordError::InvalidPort(self.port));
        }

        if SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            > self.valid_until
        {
            return Err(SeedRecordError::SeedTimeout);
        }
        Ok(())
    }

    pub fn decode(record: &str) -> Result<SeedRecord, SeedRecordError> {
        let parts = record.split(SEP).collect::<Vec<&str>>();
        if parts.len() != 5 {
            return Err(SeedRecordError::InvalidRecord);
        }

        let ip: IpAddr = parts[0]
            .parse()
            .map_err(|_| SeedRecordError::InvalidRecord)?;
        let port: u16 = parts[1]
            .parse()
            .map_err(|_| SeedRecordError::InvalidRecord)?;
        let peer_id_str = parts[2];
        let peer_id = if !peer_id_str.is_empty() {
            Some(PeerId::from_str(peer_id_str).map_err(|_| SeedRecordError::InvalidRecord)?)
        } else {
            None
        };
        let valid_until: u64 = parts[3]
            .parse()
            .map_err(|_| SeedRecordError::InvalidRecord)?;
        let sig: Vec<u8> = bs58::decode(parts[4])
            .into_vec()
            .map_err(|_| SeedRecordError::InvalidRecord)?;

        if sig.len() != 65 {
            return Err(SeedRecordError::InvalidRecord);
        }

        let recid = RecoveryId::from_i32(i32::from(sig[64]))
            .map_err(|_| SeedRecordError::InvalidSignature)?;
        let signature = RecoverableSignature::from_compact(&sig[0..64], recid)
            .map_err(|_| SeedRecordError::InvalidSignature)?;

        let data = Self::data_to_sign(ip, port, peer_id.as_ref(), valid_until);
        let hash = blake2b_256(&data);
        let message = Message::from_slice(&hash).expect("create message error");

        if let Ok(pubkey) = SECP256K1.recover(&message, &signature) {
            Ok(SeedRecord {
                ip,
                port,
                peer_id,
                valid_until,
                pubkey,
            })
        } else {
            Err(SeedRecordError::InvalidSignature)
        }
    }

    pub fn decode_with_pubkey(
        record: &str,
        pubkey: &PublicKey,
    ) -> Result<SeedRecord, SeedRecordError> {
        let seed_record = Self::decode(record)?;
        if &seed_record.pubkey != pubkey {
            Err(SeedRecordError::VerifyFailed)
        } else {
            seed_record.check()?;
            Ok(seed_record)
        }
    }

    pub fn address(&self) -> Multiaddr {
        let socket_addr = SocketAddr::new(self.ip, self.port);
        let mut multi_addr = socketaddr_to_multiaddr(socket_addr);
        if let Some(peer_id) = self.peer_id.clone() {
            multi_addr.push(Protocol::P2P(Cow::Owned(peer_id.into_bytes())));
        }
        multi_addr
    }

    pub(crate) fn data_to_sign(
        ip: IpAddr,
        port: u16,
        peer_id: Option<&PeerId>,
        valid_until: u64,
    ) -> String {
        vec![
            ip.to_string(),
            port.to_string(),
            peer_id.map(PeerId::to_base58).unwrap_or_else(String::new),
            valid_until.to_string(),
        ]
        .join(&SEP.to_string())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum SeedRecordError {
    InvalidRecord,
    InvalidIp(IpAddr),
    InvalidPort(u16),
    InvalidSignature,
    VerifyFailed,
    SeedTimeout,
    #[cfg(test)]
    KeyNotMatch,
}
