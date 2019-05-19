use crate::error::{Error, ErrorKind};
pub use secp256k1::{
    All, Error as Secp256k1Error, Message, PublicKey, RecoverableSignature, Secp256k1, SecretKey,
    Signature,
};

lazy_static! {
    pub static ref SECP256K1: Secp256k1<All> = Secp256k1::new();
}

pub fn sign(message: &Message, sk: &SecretKey) -> Signature {
    SECP256K1.sign(message, sk)
}

/// position of each sigs must according to the pk that sined the sig
/// Example 2 of 3 sigs: [s1, None, s3], pks: [pk1, pk2, pk3]
pub fn verify_m_of_n(
    message: &Message,
    m_threshold: usize,
    sigs: Vec<Option<Signature>>,
    pks: Vec<PublicKey>,
) -> Result<(), Error> {
    if sigs.len() > pks.len() {
        Err(ErrorKind::SigCountOverflow)?;
    }
    if m_threshold > sigs.len() {
        Err(ErrorKind::SigNotEnough)?;
    }
    let verified_sig_count = sigs
        .iter()
        .zip(pks.iter())
        .filter_map(|(sig, pk)| sig.and_then(|sig| SECP256K1.verify(&message, &sig, pk).ok()))
        .take(m_threshold)
        .count();
    if verified_sig_count < m_threshold {
        Err(ErrorKind::Threshold(verified_sig_count, m_threshold))?
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
        sign(message, &secret_key)
    }

    fn random_secret_key() -> SecretKey {
        let mut data = [0; 32];
        thread_rng().fill(&mut data[..]);
        loop {
            if let Ok(key) = SecretKey::from_slice(&data) {
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
                Err(ErrorKind::Threshold(2, 3)),
            ),
        ];
        for (threshold, sigs, pks, result) in test_set.into_iter() {
            let message = random_message();
            let sks: Vec<SecretKey> = (0..sigs.len())
                .into_iter()
                .map(|_| random_secret_key())
                .collect();
            let pks: Vec<PublicKey> = sks
                .iter()
                .enumerate()
                .map(|(_i, sk)| PublicKey::from_secret_key(&SECP256K1, sk))
                .take(*pks)
                .collect();
            let sigs: Vec<Option<Signature>> = sigs
                .into_iter()
                .enumerate()
                .map(|(i, valid)| {
                    if *valid {
                        Some(sign(&message, &sks[i]))
                    } else {
                        None
                    }
                })
                .collect();
            let verify_result =
                verify_m_of_n(&message, *threshold, sigs, pks).map_err(|err| err.kind());
            assert_eq!(&verify_result, result);
        }
    }

    #[test]
    fn test_2_of_3_with_wrong_signature() {
        let message = random_message();
        let sks: Vec<SecretKey> = (0..3).into_iter().map(|_| random_secret_key()).collect();
        let pks: Vec<PublicKey> = sks
            .iter()
            .map(|sk| PublicKey::from_secret_key(&SECP256K1, sk))
            .collect();
        let sigs: Vec<Option<Signature>> = vec![
            Some(sign(&message, &sks[0])),
            Some(random_signature(&message)),
            Some(sign(&message, &sks[2])),
        ];
        let verify_result = verify_m_of_n(&message, 2, sigs, pks);
        assert!(verify_result.is_ok());
    }
}
