use crate::protocols::discovery::protocol::{decode, encode, DiscoveryMessage};

#[test]
fn test_codec() {
    let msg1 = DiscoveryMessage::GetNodes {
        version: 0,
        count: 1,
        listen_port: Some(1),
    };

    let msg2 = DiscoveryMessage::GetNodes {
        version: 0,
        count: 1,
        listen_port: Some(2),
    };

    let b1 = encode(msg1.clone(), false);

    let decode1 = decode(&b1, false).unwrap();
    assert_eq!(decode1, msg1);

    let b2 = encode(msg2.clone(), false);

    let decode2 = decode(&b2, false).unwrap();
    assert_eq!(decode2, msg2);
}

#[test]
fn test_codec_v2() {
    let msg1 = DiscoveryMessage::GetNodes {
        version: 0,
        count: 1,
        listen_port: Some(1),
    };

    let msg2 = DiscoveryMessage::GetNodes {
        version: 0,
        count: 1,
        listen_port: Some(2),
    };

    let b1 = encode(msg1.clone(), true);

    let decode1 = decode(&b1, true).unwrap();
    assert_eq!(decode1, msg1);

    let b2 = encode(msg2.clone(), true);

    let decode2 = decode(&b2, true).unwrap();
    assert_eq!(decode2, msg2);
}
