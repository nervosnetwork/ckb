//! TODO(doc): @driftluo
use ckb_app_config::NetworkAlertConfig;
use ckb_logger::{debug, trace};
use ckb_multisig::secp256k1::{verify_m_of_n, Message, Pubkey, Signature};
use ckb_types::{packed, prelude::*};
use failure::Error;
use std::collections::HashSet;

/// TODO(doc): @driftluo
pub struct Verifier {
    config: NetworkAlertConfig,
    pubkeys: HashSet<Pubkey>,
}

impl Verifier {
    /// TODO(doc): @driftluo
    pub fn new(config: NetworkAlertConfig) -> Self {
        let pubkeys = config
            .public_keys
            .iter()
            .map(|raw| Pubkey::from_slice(raw.as_bytes()))
            .collect::<Result<HashSet<Pubkey>, _>>()
            .expect("builtin pubkeys");
        Verifier { config, pubkeys }
    }

    /// TODO(doc): @driftluo
    pub fn verify_signatures(&self, alert: &packed::Alert) -> Result<(), Error> {
        trace!("verify alert {:?}", alert);
        let message = Message::from_slice(alert.calc_alert_hash().as_slice())?;
        let signatures: Vec<Signature> = alert
            .signatures()
            .into_iter()
            .filter_map(
                |sig_data| match Signature::from_slice(sig_data.as_reader().raw_data()) {
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
                },
            )
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
