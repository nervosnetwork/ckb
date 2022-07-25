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

    let b1 = encode(msg1.clone());

    let decode1 = decode(&b1).unwrap();
    assert_eq!(decode1, msg1);

    let b2 = encode(msg2.clone());

    let decode2 = decode(&b2).unwrap();
    assert_eq!(decode2, msg2);
}
