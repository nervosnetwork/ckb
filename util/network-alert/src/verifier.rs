use crate::config::SignatureConfig;
use ckb_logger::{debug, trace};
use ckb_multisig::secp256k1::{verify_m_of_n, Message, Pubkey, Signature};
use ckb_types::packed;
use failure::Error;
use std::collections::HashSet;

pub struct Verifier {
    config: SignatureConfig,
    pubkeys: HashSet<Pubkey>,
}

impl Verifier {
    pub fn new(config: SignatureConfig) -> Self {
        let pubkeys = config
            .public_keys
            .iter()
            .map(|raw| Pubkey::from_slice(raw.as_bytes()))
            .collect::<Result<HashSet<Pubkey>, _>>()
            .expect("builtin pubkeys");
        Verifier { config, pubkeys }
    }

    pub fn verify_signatures(&self, alert: &packed::Alert) -> Result<(), Error> {
        trace!("verify alert {:?}", alert);
        let message = Message::from_slice(alert.calc_alert_hash().as_bytes())?;
        let signatures: Vec<Signature> = alert
            .signatures()
            .into_iter()
            .filter_map(|sig_data| {
                match Signature::from_slice(sig_data.as_reader().as_unpack_slice()) {
                    Ok(sig) => {
                        if sig.is_valid() {
                            Some(sig)
                        } else {
                            debug!("invalid signature: {:?}", sig);
                            None
                        }
                    }
                    Err(err) => {
                        debug!("signature error: {}", err);
                        None
                    }
                }
            })
            .collect();
        verify_m_of_n(
            &message,
            self.config.signatures_threshold,
            &signatures,
            &self.pubkeys,
        )
        .map_err(|err| err.kind())?;
        Ok(())
    }
}
