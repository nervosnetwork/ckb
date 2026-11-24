use crate::network::yamux_max_stream_count;
use ckb_app_config::default_support_all_protocols;

#[test]
fn stream_limit_covers_the_full_default_protocol_list() {
    // A peer on the default list opens one stream per protocol, so a receiver
    // with the same list must be able to accept all of them plus the extra
    // streams used while the peer's unsupported protocols are negotiated.
    let default_count = default_support_all_protocols().len();
    assert!(
        yamux_max_stream_count(default_count) > default_count,
        "the limit must leave headroom for the inbound streams of a dialer"
    );
}

#[test]
fn stream_limit_does_not_shrink_with_a_shorter_local_protocol_list() {
    // Dropping an optional protocol from the local config is supported, and it
    // used to shrink the limit below what the dialer sends (#5341).
    let default_count = default_support_all_protocols().len();
    for removed in 1..default_count {
        let local_count = default_count - removed;
        assert_eq!(
            yamux_max_stream_count(local_count),
            yamux_max_stream_count(default_count),
            "a local list of {local_count} protocols must keep the same capacity"
        );
    }
}

#[test]
fn stream_limit_grows_for_a_larger_local_protocol_list() {
    // Tests and downstream users can register extra CKB protocols; those need
    // room as well, so the limit never falls below the local count.
    let default_count = default_support_all_protocols().len();
    let local_count = default_count + 5;
    assert!(yamux_max_stream_count(local_count) >= local_count);
    assert!(yamux_max_stream_count(local_count) > yamux_max_stream_count(default_count));
}
