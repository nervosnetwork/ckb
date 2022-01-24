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
            match sig.recover(message) {
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
