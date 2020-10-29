//! Multi-signatures using secp256k1
use crate::error::{Error, ErrorKind};
pub use ckb_crypto::secp::{Error as Secp256k1Error, Message, Privkey, Pubkey, Signature};
use ckb_logger::{debug, trace};
use std::collections::HashSet;
use std::hash::BuildHasher;

/// Verifies m of n signatures.
///
/// Example 2 of 3 sigs: [s1, s3], pks: [pk1, pk2, pk3]
pub fn verify_m_of_n<S>(
    message: &Message,
    m_threshold: usize,
    sigs: &[Signature],
    pks: &HashSet<Pubkey, S>,
) -> Result<(), Error>
where
    S: BuildHasher,
{
    if sigs.len() > pks.len() {
        return Err(ErrorKind::SigCountOverflow.into());
    }
    if m_threshold > sigs.len() {
        return Err(ErrorKind::SigNotEnough.into());
    }

    let mut used_pks: HashSet<Pubkey> = HashSet::with_capacity(m_threshold);
    let verified_sig_count = sigs
        .iter()
        .filter_map(|sig| {
            trace!(
                "recover sig {:x?} with message {:x?}",
                &sig.serialize()[..],
                message.as_ref()
            );
            match sig.recover(&message) {
                Ok(pubkey) => Some(pubkey),
                Err(err) => {
                    debug!("recover secp256k1 sig error: {}", err);
                    None
                }
            }
        })
        .filter(|rec_pk| pks.contains(rec_pk) && used_pks.insert(rec_pk.to_owned()))
        .take(m_threshold)
        .count();
    if verified_sig_count < m_threshold {
        return Err(ErrorKind::Threshold {
            pass_sigs: verified_sig_count,
            threshold: m_threshold,
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, Rng};

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
}
