use crate::verifier::Verifier;
use ckb_app_config::NetworkAlertConfig;
use ckb_crypto::secp::Generator;
use ckb_jsonrpc_types::JsonBytes;
use ckb_types::{packed, prelude::*};

#[test]
fn test_veirifer() {
    let keypairs: Vec<_> = (0..3).map(move |_| Generator::random_keypair()).collect();
    let config = NetworkAlertConfig {
        signatures_threshold: 2,
        public_keys: keypairs
            .iter()
            .map(|(_, pubkey)| JsonBytes::from_vec(pubkey.serialize()))
            .collect(),
    };
    let verifier = Verifier::new(config);
    let raw_alert = packed::RawAlert::new_builder().id(1u32.pack()).build();
    let hash = raw_alert.calc_alert_hash();
    let signatures = keypairs
        .iter()
        .map(|(privkey, _)| privkey.sign_recoverable(&hash.unpack()))
        .collect::<Result<Vec<_>, _>>()
        .expect("sign alert")
        .iter()
        .map(|sig| sig.serialize().pack())
        .fold(packed::BytesVec::new_builder(), |builder, item| {
            builder.push(item)
        })
        .build();
    let alert = packed::Alert::new_builder()
        .raw(raw_alert)
        .signatures(signatures)
        .build();
    assert!(verifier.verify_signatures(&alert).is_ok());
}
