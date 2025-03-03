//! verify module
//!
//! The message of this protocol must be verified by multi-signature before notifying the user.
//! The implementation of any client must be consistent with ckb to prevent useless information from being broadcast on the entire network.
//! The set of public keys is currently in the possession of the Nervos foundation
//!
use ckb_app_config::NetworkAlertConfig;
use ckb_error::AnyError;
use ckb_logger::{debug, trace};
use ckb_multisig::secp256k1::{Message, Pubkey, Signature, verify_m_of_n};
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;

/// Message verify
pub struct Verifier {
    config: NetworkAlertConfig,
    pubkeys: HashSet<Pubkey>,
}

impl Verifier {
    /// Init with ckb alert config
    pub fn new(config: NetworkAlertConfig) -> Self {
        let pubkeys = config
            .public_keys
            .iter()
            .map(|raw| Pubkey::from_slice(raw.as_bytes()))
            .collect::<Result<HashSet<Pubkey>, _>>()
            .expect("builtin pubkeys");
        Verifier { config, pubkeys }
    }

    /// Verify signatures
    pub fn verify_signatures(&self, alert: &packed::Alert) -> Result<(), AnyError> {
        trace!("Verifying alert {:?}", alert);
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
