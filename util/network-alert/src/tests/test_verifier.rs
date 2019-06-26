use crate::config::Config;
use crate::verifier::Verifier;
use ckb_core::alert::AlertBuilder;
use ckb_crypto::secp::Generator;
use ckb_jsonrpc_types::JsonBytes;

#[test]
fn test_veirifer() {
    let keypairs: Vec<_> = (0..3)
        .map(move |_| Generator::new().random_keypair())
        .collect();
    let config = Config {
        signatures_threshold: 2,
        public_keys: keypairs
            .iter()
            .map(|(_, pubkey)| JsonBytes::from_vec(pubkey.serialize()))
            .collect(),
    };
    let verifier = Verifier::new(config);
    let mut alert = AlertBuilder::default().id(1).build();
    let hash = alert.hash();
    let signatures = keypairs
        .iter()
        .map(|(privkey, _)| privkey.sign_recoverable(&hash))
        .collect::<Result<Vec<_>, _>>()
        .expect("sign alert");
    alert.signatures = signatures
        .into_iter()
        .map(|s| s.serialize().into())
        .collect();
    assert!(verifier.verify_signatures(&alert).is_ok());
}
