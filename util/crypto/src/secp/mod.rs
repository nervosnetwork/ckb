#![allow(dead_code)]

use lazy_static::lazy_static;
use numext_fixed_hash::H256;

pub type Message = H256;

lazy_static! {
    pub static ref SECP256K1: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
}

mod error;
mod generator;
mod privkey;
mod pubkey;
mod signature;

pub use self::error::Error;
pub use self::generator::Generator;
pub use self::privkey::Privkey;
pub use self::pubkey::Pubkey;
pub use self::signature::Signature;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{self, Rng};

    fn random_message() -> Message {
        let mut message = Message::default();
        let mut rng = rand::thread_rng();
        rng.fill(message.as_mut());
        message
    }

    #[test]
    fn test_gen_keypair() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair();
        assert_eq!(privkey.pubkey().expect("pubkey"), pubkey);
    }

    #[test]
    fn test_sign_verify() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair();
        let message = random_message();
        let signature = privkey.sign_recoverable(&message).unwrap();
        assert!(signature.is_valid());
        assert!(pubkey.verify(&message, &signature).is_ok());
    }

    #[test]
    fn test_recover() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair();
        let message = random_message();
        let signature = privkey.sign_recoverable(&message).unwrap();
        assert_eq!(pubkey, signature.recover(&message).unwrap());
    }

    #[test]
    fn test_serialize() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair();
        let ser_pubkey = privkey.pubkey().expect("pubkey").serialize();
        assert_eq!(ser_pubkey.len(), 33);
        let deser_pubkey = Pubkey::from_slice(&ser_pubkey).expect("deserialize pubkey");
        assert_eq!(deser_pubkey, pubkey);

        let deser_privkey = Privkey::from_slice(privkey.as_ref());
        assert_eq!(deser_privkey, privkey);

        let msg = random_message();
        let signature = privkey.sign_recoverable(&msg).expect("sign");
        let ser_signature = signature.serialize();
        assert_eq!(ser_signature.len(), 65);
        let deser_signature = Signature::from_slice(&ser_signature).expect("deserialize");
        assert!(deser_signature.is_valid());
        assert_eq!(ser_signature, deser_signature.serialize());
    }

}
