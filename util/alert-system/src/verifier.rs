use crate::config::Config;
use ckb_core::alert::Alert;
use failure::Error;
use log::{debug, trace};

pub struct Verifier(Config);

impl Verifier {
    pub fn new(config: Config) -> Self {
        Verifier(config)
    }

    pub fn verify_signatures(&self, alert: &Alert) -> Result<(), Error> {
        trace!(target: "alert", "verify alert {:?}", alert);
        use multisig::secp256k1::{verify_m_of_n, Message, PublicKey, Signature};
        let message = Message::from_slice(alert.hash().as_bytes())?;
        let signatures: Vec<Option<Signature>> = alert
            .signatures
            .iter()
            .map(|sig| {
                if sig.is_empty() {
                    None
                } else {
                    match Signature::from_compact(sig) {
                        Ok(i) => Some(i),
                        Err(err) => {
                            debug!(target: "alert", "signature error: {}", err);
                            None
                        }
                    }
                }
            })
            .collect();
        let public_keys = self
            .0
            .public_keys
            .iter()
            .map(|raw| PublicKey::from_slice(raw.as_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;
        verify_m_of_n(
            &message,
            self.0.signatures_threshold,
            signatures,
            public_keys,
        )
        .map_err(|err| err.kind())?;
        Ok(())
    }
}
