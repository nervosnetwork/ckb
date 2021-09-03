use ckb_hash::blake2b_256;
use p2p::secio::PeerId;
use rand::Rng;
use secp256k1::{
    key::{PublicKey, SecretKey},
    Message,
};
use std::{
    net::IpAddr,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::services::dns_seeding::seed_record::{SeedRecord, SeedRecordError, SECP256K1, SEP};

struct Generator;

impl Generator {
    fn random_keypair() -> (SecretKey, PublicKey) {
        let secret_key = Self::random_secret_key();
        let pubkey = PublicKey::from_secret_key(&*SECP256K1, &secret_key);
        (secret_key, pubkey)
    }

    fn random_secret_key() -> SecretKey {
        let mut seed = vec![0; 32];
        let mut rng = rand::thread_rng();
        loop {
            rng.fill(seed.as_mut_slice());
            if let Ok(key) = SecretKey::from_slice(&seed) {
                return key;
            }
        }
    }
}

impl SeedRecord {
    fn new(
        ip: IpAddr,
        port: u16,
        peer_id: Option<PeerId>,
        valid_until: u64,
        pubkey: PublicKey,
    ) -> SeedRecord {
        SeedRecord {
            ip,
            port,
            peer_id,
            valid_until,
            pubkey,
        }
    }

    // Design for human readable
    fn encode(&self, privkey: &SecretKey) -> Result<String, SeedRecordError> {
        if PublicKey::from_secret_key(&SECP256K1, privkey) != self.pubkey {
            return Err(SeedRecordError::KeyNotMatch);
        }

        let data = Self::data_to_sign(self.ip, self.port, self.peer_id.as_ref(), self.valid_until);
        let hash = blake2b_256(&data);
        let message = Message::from_slice(&hash).expect("create message error");

        let signature = SECP256K1.sign_recoverable(&message, privkey);
        let (recid, signed_data) = signature.serialize_compact();
        let mut sig = [0u8; 65];
        sig[0..64].copy_from_slice(&signed_data[0..64]);
        sig[64] = recid.to_i32() as u8;
        let signature_string = bs58::encode(&sig[..]).into_string();
        Ok(vec![data, signature_string].join(&SEP.to_string()))
    }
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

#[test]
fn simple() {
    let ipv4: IpAddr = "153.149.96.217".parse().unwrap();
    let port = 4455;
    let peer_id = Some(PeerId::random());
    // 180 seconds in future
    let valid_until = now_ts() + 180;
    let (priv1, pub1) = Generator::random_keypair();
    let (priv2, pub2) = Generator::random_keypair();
    let record = SeedRecord::new(ipv4, port, peer_id.clone(), valid_until, pub1);
    assert_eq!(record.encode(&priv2), Err(SeedRecordError::KeyNotMatch));
    let record_string = record.encode(&priv1).unwrap();
    let ret = SeedRecord::decode(record_string.as_str());
    assert!(ret.is_ok());
    let record = ret.unwrap();
    assert!(record.check().is_ok());
    assert!(record.port == 4455);
    assert!(record.pubkey != pub2);

    let ipv6: IpAddr = "2001:0dc5:72a3:0000:0000:802e:3370:73E4".parse().unwrap();
    let record = SeedRecord::new(ipv6, port, peer_id, valid_until, pub1);
    assert!(record.check().is_ok());
}
