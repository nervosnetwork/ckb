use crate::{
    multiaddr::Multiaddr,
    peer_store::{
        types::{multiaddr_to_ip_network, AddrInfo, BannedAddr, MultiaddrExt},
        PeerStore,
    },
    PeerId,
};

use std::collections::HashSet;

#[test]
fn test_peer_store_persistent() {
    let now_ms = faketime::unix_time_as_millis();
    let mut peer_store = PeerStore::default();

    // add addrs to addr manager
    let addr_manager = peer_store.mut_addr_manager();
    let addr1 = {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/42".parse().unwrap();
        AddrInfo::new(
            PeerId::random(),
            addr.extract_ip_addr().unwrap(),
            addr,
            0,
            60,
        )
    };
    let addr2 = {
        let addr: Multiaddr = "/ip4/127.0.0.5/tcp/42".parse().unwrap();
        let mut addr_info = AddrInfo::new(
            PeerId::random(),
            addr.extract_ip_addr().unwrap(),
            addr,
            100,
            30,
        );
        addr_info.mark_tried(now_ms);
        addr_info
    };
    addr_manager.add(addr1.clone());
    addr_manager.add(addr2.clone());

    // add addrs to ban list
    let ban_list = peer_store.mut_ban_list();
    let addr3 = multiaddr_to_ip_network(&"/ip4/127.0.0.1/tcp/42".parse().unwrap()).unwrap();
    let addr4 = multiaddr_to_ip_network(&"/ip4/127.0.0.2/tcp/42".parse().unwrap()).unwrap();
    let addr5 = multiaddr_to_ip_network(&"/ip4/255.0.0.1/tcp/42".parse().unwrap()).unwrap();
    let ban1 = BannedAddr {
        address: addr3,
        ban_until: now_ms + 10_000,
        ban_reason: "test1".into(),
        created_at: now_ms,
    };
    let ban2 = BannedAddr {
        address: addr4,
        ban_until: now_ms + 20_000,
        ban_reason: "test2".into(),
        created_at: now_ms + 1,
    };
    let ban3 = BannedAddr {
        address: addr5,
        ban_until: now_ms + 30_000,
        ban_reason: "test3".into(),
        created_at: now_ms + 2,
    };
    ban_list.ban(ban1.clone());
    ban_list.ban(ban2.clone());
    ban_list.ban(ban3.clone());

    // dump and load
    let dir = tempfile::tempdir().unwrap();
    peer_store.dump_to_dir(&dir.path()).unwrap();
    let peer_store2 = PeerStore::load_from_dir(&dir.path()).unwrap();

    // check addr manager
    let addr_manager2 = peer_store2.addr_manager();
    // set random_id_pos to default, this field is internal used only
    let addrs = addr_manager2.addrs_iter().cloned().map(|mut paddr| {
        paddr.random_id_pos = 0;
        paddr
    });
    assert_eq!(
        addrs.collect::<HashSet<_>>(),
        vec![addr1, addr2].into_iter().collect::<HashSet<_>>()
    );

    // check ban list
    let ban_list2 = peer_store2.ban_list();
    assert_eq!(
        ban_list2
            .get_banned_addrs()
            .into_iter()
            .collect::<HashSet<_>>(),
        vec![ban1, ban2, ban3].into_iter().collect::<HashSet<_>>()
    );
}
