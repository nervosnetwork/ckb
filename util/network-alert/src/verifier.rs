use crate::config::Config;
use ckb_core::alert::Alert;
use ckb_logger::{debug, trace};
use ckb_multisig::secp256k1::{verify_m_of_n, Message, Pubkey, Signature};
use failure::Error;
use fnv::FnvHashSet;

pub struct Verifier {
    config: Config,
    pubkeys: FnvHashSet<Pubkey>,
}

impl Verifier {
    pub fn new(config: Config) -> Self {
        let pubkeys = config
            .public_keys
            .iter()
            .map(|raw| Pubkey::from_slice(raw.as_bytes()))
            .collect::<Result<FnvHashSet<Pubkey>, _>>()
            .expect("builtin pubkeys");
        Verifier { config, pubkeys }
    }

    pub fn verify_signatures(&self, alert: &Alert) -> Result<(), Error> {
        trace!("verify alert {:?}", alert);
        let message = Message::from_slice(alert.hash().as_bytes())?;
        let signatures: Vec<Signature> = alert
            .signatures
            .iter()
            .filter_map(|sig_data| match Signature::from_slice(sig_data) {
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
