use crate::verifier::Verifier;
use ckb_app_config::NetworkAlertConfig;
use ckb_crypto::secp::Privkey;
use ckb_jsonrpc_types::{Alert, JsonBytes};
use ckb_types::{packed, prelude::*};
use faster_hex::hex_decode;

const DUMMY_PUBKEY: &str = "0329e3889cae7b1788836ff8841646174aeb7828a4f997c7f8e54c7257c9ff21a3";

// This is a test privkey, do not use it as your own account.
// To create a signature, replace this with your privkey and DO NOT COMMIT THE CHANGE.
const DUMMY_PRIVKEY: &str = "3b3b6f014ceb07c8a14dc2d5bb08f3ee6975811bbdd0d8d53fe2c9e69ef4e498";

#[test]
fn test_signing_alert_using_dummy_keypair() {
    let mut config = NetworkAlertConfig {
        signatures_threshold: 1,
        ..Default::default()
    };

    let mut pubkey_buffer = vec![0u8; DUMMY_PUBKEY.len() / 2];
    hex_decode(DUMMY_PUBKEY.as_bytes(), &mut pubkey_buffer).expect("valid hex");
    config.public_keys.push(JsonBytes::from_vec(pubkey_buffer));

    let verifier = Verifier::new(config);
    let raw_alert = packed::RawAlert::new_builder()
        // 3 months later
        .notice_until(1681574400000u64.pack())
        .id(20230001u32.pack())
        .cancel(0u32.pack())
        .priority(20u32.pack())
        .message("CKB v0.105.* have bugs. Please upgrade to the latest version.".pack())
        .min_version(Some("0.105.0-pre").pack())
        .max_version(Some("0.105.1").pack())
        .build();

    let hash = raw_alert.calc_alert_hash();
    let privkey = DUMMY_PRIVKEY.parse::<Privkey>().unwrap();
    let signature = privkey
        .sign_recoverable(&hash.unpack())
        .unwrap()
        .serialize()
        .pack();
    let alert = packed::Alert::new_builder()
        .raw(raw_alert)
        .signatures(vec![signature.clone()].pack())
        .build();
    let alert_json = Alert::from(alert.clone());
    println!(
        "alert:\n{}",
        serde_json::to_string_pretty(&alert_json).unwrap()
    );
    println!("raw hash: 0x{:x}", hash);
    println!("signature: 0x{:x}", signature.raw_data());
    assert!(verifier.verify_signatures(&alert).is_ok());
}

#[test]
fn test_alert_20230001() {
    let config = NetworkAlertConfig::default();
    let verifier = Verifier::new(config);
    let raw_alert = packed::RawAlert::new_builder()
        // 3 months later
        .notice_until(1681574400000u64.pack())
        .id(20230001u32.pack())
        .cancel(0u32.pack())
        .priority(20u32.pack())
        .message("CKB v0.105.* have bugs. Please upgrade to the latest version.".pack())
        .min_version(Some("0.105.0-pre").pack())
        .max_version(Some("0.105.1").pack())
        .build();

    let signatures = vec![
        "8dca283684ff3cd024bd6a67efb24617e90e31dc69ac809ac6ac5e243a57b7aa6711228dfbd8a5cc89a68d3065b685e5c56c70740e8d3487fd538dc914d0c97c00",
        "4554b37824e17ea02432507e372c869301a415bf718e0a5a33b6df75cd32fbab7cf8176ca8b079c28266ce1f33c3f61fbff19e27be2a85f5a14faa2b1b474e0a01"
    ].iter().map(|hex| {
        let mut buf = vec![0u8; hex.len() / 2];
        hex_decode(hex.as_bytes(), &mut buf).expect("valid hex");
        buf.pack()
    }).fold(packed::BytesVec::new_builder(), |builder, item| {
        builder.push(item)
    }).build();
    let alert = packed::Alert::new_builder()
        .raw(raw_alert)
        .signatures(signatures)
        .build();
    let alert_json = Alert::from(alert.clone());
    println!(
        "alert:\n{}",
        serde_json::to_string_pretty(&alert_json).unwrap()
    );
    assert!(verifier.verify_signatures(&alert).is_ok());
}
