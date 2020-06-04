mod alert_propagation;

pub use alert_propagation::*;

use ckb_app_config::NetworkAlertConfig;
use ckb_crypto::secp::Privkey;
use ckb_jsonrpc_types::JsonBytes;
use rand::{thread_rng, Rng};

pub(crate) fn random_privkey() -> Privkey {
    let mut rng = thread_rng();
    let mut raw = [0; 32];
    loop {
        rng.fill(&mut raw);
        let privkey = Privkey::from_slice(&raw[..]);
        if privkey.pubkey().is_ok() {
            return privkey;
        }
    }
}

pub(crate) fn new_alert_config(
    signatures_threshold: usize,
    key_num: usize,
) -> (NetworkAlertConfig, Vec<Privkey>) {
    let privkeys: Vec<_> = (0..key_num).map(|_| random_privkey()).collect();
    let alert_config = NetworkAlertConfig {
        signatures_threshold,
        public_keys: privkeys
            .iter()
            .map(|privkey| {
                let pubkey = privkey.pubkey().expect("pubkey");
                JsonBytes::from_vec(pubkey.serialize())
            })
            .collect(),
    };
    (alert_config, privkeys)
}
