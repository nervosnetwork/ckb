use rand::{self, Rng};

use crate::secp::{Generator, Message, Privkey, Pubkey, Signature};

fn random_message() -> Message {
    let mut message = Message::default();
    let mut rng = rand::thread_rng();
    rng.fill(message.as_mut());
    message
}

#[test]
fn test_gen_keypair() {
    let (privkey, pubkey) = Generator::random_keypair();
    assert_eq!(privkey.pubkey().expect("pubkey"), pubkey);
}

#[test]
fn test_sign_verify() {
    let (privkey, pubkey) = Generator::random_keypair();
    let message = random_message();
    let signature = privkey.sign_recoverable(&message).unwrap();
    assert!(signature.is_valid());
    assert!(pubkey.verify(&message, &signature).is_ok());
}

#[test]
fn test_recover() {
    let (privkey, pubkey) = Generator::random_keypair();
    let message = random_message();
    let signature = privkey.sign_recoverable(&message).unwrap();
    assert_eq!(pubkey, signature.recover(&message).unwrap());
}

#[test]
fn test_serialize() {
    let (privkey, pubkey) = Generator::random_keypair();
    let ser_pubkey = privkey.pubkey().expect("pubkey").serialize();
    assert_eq!(ser_pubkey.len(), 33);
    let deser_pubkey = Pubkey::from_slice(&ser_pubkey).expect("deserialize pubkey");
    assert_eq!(deser_pubkey, pubkey);

    let msg = random_message();
    let signature = privkey.sign_recoverable(&msg).expect("sign");
    let ser_signature = signature.serialize();
    assert_eq!(ser_signature.len(), 65);
    let deser_signature = Signature::from_slice(&ser_signature).expect("deserialize");
    assert!(deser_signature.is_valid());
    assert_eq!(ser_signature, deser_signature.serialize());
}

#[test]
fn privkey_zeroize() {
    let (mut privkey, _) = Generator::random_keypair();
    privkey.zeroize();
    assert!(privkey == Privkey::from_slice([0u8; 32].as_ref()));
}
