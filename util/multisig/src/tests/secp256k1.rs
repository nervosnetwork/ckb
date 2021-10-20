use ckb_crypto::secp::{Message, Privkey, Pubkey, Signature};
use rand::{thread_rng, Rng};
use std::collections::HashSet;

use crate::{error::ErrorKind, secp256k1::verify_m_of_n};

fn random_message() -> Message {
    let mut data = [0; 32];
    thread_rng().fill(&mut data[..]);
    loop {
        if let Ok(msg) = Message::from_slice(&data) {
            return msg;
        }
    }
}

fn random_signature(message: &Message) -> Signature {
    let secret_key = random_secret_key();
    secret_key.sign_recoverable(message).expect("sign")
}

fn random_secret_key() -> Privkey {
    let mut data = [0; 32];
    thread_rng().fill(&mut data[..]);
    loop {
        let key = Privkey::from_slice(&data);
        if key.pubkey().is_ok() {
            return key;
        }
    }
}

#[test]
fn test_m_of_n() {
    // (thresholds, sigs: [is_valid], pks, result)
    let test_set = [
        (2, vec![true, true], 3, Ok(())),
        (2, vec![true, true, true], 3, Ok(())),
        (3, vec![true, true, true], 3, Ok(())),
        (3, vec![true, false, true, true], 4, Ok(())),
        (
            2,
            vec![true, true, true],
            1,
            Err(ErrorKind::SigCountOverflow),
        ),
        (3, vec![true, true], 3, Err(ErrorKind::SigNotEnough)),
        (
            3,
            vec![true, true, false],
            3,
            Err(ErrorKind::Threshold {
                pass_sigs: 2,
                threshold: 3,
            }),
        ),
    ];
    for (threshold, sigs, pks, result) in test_set.iter() {
        let message = random_message();
        let sks: Vec<Privkey> = (0..sigs.len()).map(|_| random_secret_key()).collect();
        let pks: HashSet<Pubkey> = sks
            .iter()
            .enumerate()
            .map(|(_i, sk)| sk.pubkey().expect("pk"))
            .take(*pks)
            .collect();
        let sigs: Vec<Signature> = sigs
            .iter()
            .enumerate()
            .map(|(i, valid)| {
                if *valid {
                    sks[i].sign_recoverable(&message).expect("sign")
                } else {
                    random_signature(&message)
                }
            })
            .collect();
        let verify_result =
            verify_m_of_n(&message, *threshold, &sigs, &pks).map_err(|err| err.kind());
        assert_eq!(&verify_result, result);
    }
}

#[test]
fn test_2_of_3_with_wrong_signature() {
    let message = random_message();
    let sks: Vec<Privkey> = (0..3).map(|_| random_secret_key()).collect();
    let pks: HashSet<Pubkey> = sks.iter().map(|sk| sk.pubkey().expect("pk")).collect();
    let sigs: Vec<Signature> = vec![
        sks[0].sign_recoverable(&message).expect("sign"),
        sks[2].sign_recoverable(&message).expect("sign"),
        random_signature(&message),
    ];
    let verify_result = verify_m_of_n(&message, 2, &sigs, &pks);
    assert!(verify_result.is_ok());
}

#[test]
fn test_duplicate_pubkeys() {
    let message = random_message();
    let sks: Vec<Privkey> = (0..3).map(|_| random_secret_key()).collect();
    let pks: HashSet<Pubkey> = sks.iter().map(|sk| sk.pubkey().expect("pk")).collect();
    let sigs: Vec<Signature> = vec![
        sks[0].sign_recoverable(&message).expect("sign"),
        sks[0].sign_recoverable(&message).expect("sign"),
        random_signature(&message),
    ];
    let verify_result = verify_m_of_n(&message, 2, &sigs, &pks).map_err(|err| err.kind());
    assert_eq!(
        verify_result,
        Err(ErrorKind::Threshold {
            pass_sigs: 1,
            threshold: 2
        })
    );
}
