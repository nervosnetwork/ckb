use crate::{
    multiaddr::Multiaddr,
    peer_store::{addr_manager::AddrManager, types::AddrInfo},
    PeerId,
};
use proptest::prelude::*;
use std::net::Ipv4Addr;

const MAX_FETCHED_ADDRS: usize = 1000;
const RANDOM_REMOVE_ADDRS: usize = 500;
proptest! {
    #[test]
    fn test_add_random_addrs(count in RANDOM_REMOVE_ADDRS..MAX_FETCHED_ADDRS) {
        fn new_addr(id: usize) -> AddrInfo{
            let ip = Ipv4Addr::from(((225 << 24) + id) as u32);
            let addr: Multiaddr = format!("/ip4/{}/tcp/42/p2p/{}", ip, PeerId::random().to_base58()).parse().unwrap();
            AddrInfo::new(
                addr,
                0,
                0,
            )
        }
        let mut addr_manager: AddrManager = Default::default();
        for i in 0..count {
            addr_manager.add(new_addr(i + 1));
        }
        // randomly remove addrs
        let removed_addrs = addr_manager.fetch_random(RANDOM_REMOVE_ADDRS, |_| true);
        assert_eq!(removed_addrs.len(), RANDOM_REMOVE_ADDRS);
        for addr in &removed_addrs {
            addr_manager.remove(&addr.addr);
        }
        assert_eq!(addr_manager.count(), count - RANDOM_REMOVE_ADDRS);
        // add back removed addrs
        for addr in removed_addrs {
            addr_manager.add(addr);
        }
        let addrs = addr_manager.fetch_random(count + 1, |_| true);
        assert_eq!(addrs.len(), count);
    }
}
