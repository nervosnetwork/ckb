use crate::{
    multiaddr::Multiaddr,
    peer_store::{
        addr_manager::AddrManager,
        ban_list::BanList,
        types::{multiaddr_to_ip_network, AddrInfo, BannedAddr, MultiaddrExt},
    },
    PeerId,
};

use std::collections::HashSet;

#[test]
fn test_ban_list() {
    let now_ms = faketime::unix_time_as_millis();
    let mut ban_list = BanList::default();
    let addr1 = multiaddr_to_ip_network(&"/ip4/127.0.0.1/tcp/42".parse().unwrap()).unwrap();
    let addr2 = multiaddr_to_ip_network(&"/ip4/127.0.0.2/tcp/42".parse().unwrap()).unwrap();
    let addr3 = multiaddr_to_ip_network(&"/ip4/255.0.0.1/tcp/42".parse().unwrap()).unwrap();
    let ban1 = BannedAddr {
        address: addr1,
        ban_until: now_ms + 10_000,
        ban_reason: "test1".into(),
        created_at: now_ms,
    };
    let ban2 = BannedAddr {
        address: addr2,
        ban_until: now_ms + 20_000,
        ban_reason: "test2".into(),
        created_at: now_ms + 1,
    };
    let ban3 = BannedAddr {
        address: addr3,
        ban_until: now_ms + 30_000,
        ban_reason: "test3".into(),
        created_at: now_ms + 2,
    };
    ban_list.ban(ban1.clone());
    ban_list.ban(ban2.clone());
    ban_list.ban(ban3.clone());
    let path = tempfile::tempdir().unwrap().path().join("ban_list.db");
    ban_list.dump(&path).unwrap();
    let ban_list2 = BanList::load(&path).unwrap();
    assert_eq!(
        ban_list2
            .get_banned_addrs()
            .into_iter()
            .collect::<HashSet<_>>(),
        vec![ban1, ban2, ban3].into_iter().collect::<HashSet<_>>()
    );
}

#[test]
fn test_add_addr() {
    let now_ms = faketime::unix_time_as_millis();
    let mut addr_manager = AddrManager::default();
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
    let path = tempfile::tempdir().unwrap().path().join("addr_manager.db");
    addr_manager.dump(&path).unwrap();
    let addr_manager2 = AddrManager::load(&path).unwrap();
    // set random_id_pos to default, this field is internal used only
    let addrs = addr_manager2.addrs_iter().cloned().map(|mut paddr| {
        paddr.random_id_pos = 0;
        paddr
    });
    assert_eq!(
        addrs.collect::<HashSet<_>>(),
        vec![addr1, addr2].into_iter().collect::<HashSet<_>>()
    );
}
