use crate::{
    multiaddr::Multiaddr,
    peer_store::{
        types::{multiaddr_to_ip_network, AddrInfo, BannedAddr, MultiaddrExt},
        PeerStore,
    },
    PeerId,
};

use std::fs::File;
use std::io::Write;
use std::{collections::HashSet, fs::create_dir_all};

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
    let peer_store2 = PeerStore::load_from_dir_or_default(&dir.path());

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

#[test]
fn test_peer_store_load_from_dir_should_not_panic() {
    // should return an empty store when dir does not exist
    {
        let peer_store = PeerStore::load_from_dir_or_default("/tmp/a_directory_does_not_exist");
        assert_eq!(0, peer_store.addr_manager().count());
        assert_eq!(0, peer_store.ban_list().get_banned_addrs().len());
    }

    // should return an empty store when AddrManager db is empty or broken
    {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("addr_manager.db");
        let mut file = File::create(file_path).unwrap();
        writeln!(file).unwrap();
        let peer_store = PeerStore::load_from_dir_or_default(dir);
        assert_eq!(0, peer_store.addr_manager().count());
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("addr_manager.db");
        let mut file = File::create(file_path).unwrap();
        writeln!(file, "broken").unwrap();
        let peer_store = PeerStore::load_from_dir_or_default(dir);
        assert_eq!(0, peer_store.addr_manager().count());
    }

    // should return an empty store when BanList db is empty or broken
    {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("ban_list.db");
        let mut file = File::create(file_path).unwrap();
        writeln!(file).unwrap();
        let peer_store = PeerStore::load_from_dir_or_default(dir);
        assert_eq!(0, peer_store.ban_list().get_banned_addrs().len());
    }
    {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("ban_list.db");
        let mut file = File::create(file_path).unwrap();
        writeln!(file, "broken").unwrap();
        let peer_store = PeerStore::load_from_dir_or_default(dir);
        assert_eq!(0, peer_store.ban_list().get_banned_addrs().len());
    }
}

#[test]
fn test_peer_store_dump_with_broken_tmp_file_should_be_ok() {
    let dir = tempfile::tempdir().unwrap();
    create_dir_all(dir.path().join("tmp")).unwrap();
    // write a truncated json with 8 peers to tmp file
    {
        let tmp_addr_file_path = dir.path().join("tmp/addr_manager.db");
        let mut file = File::create(tmp_addr_file_path).unwrap();
        let truncated_json = r#"[{"peer_id":"QmZDfQQPzQmPXW8DjoCw9QVLJU85rnSxgH3j3u4j19hq4o","ip_port":{"ip":"127.0.0.1","port":438},"addr":"/ip4/127.0.0.1/tcp/438","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":438},{"peer_id":"Qmb34eHAHK4BgSCNQ5KV3Jc6iqh3FBRWEzkT6M4Yztoac6","ip_port":{"ip":"127.0.0.1","port":46},"addr":"/ip4/127.0.0.1/tcp/46","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":46},{"peer_id":"QmeKJZ2AgE3tbP5hjeKeoLGArApQDcgXenKJ3w5eG48esg","ip_port":{"ip":"127.0.0.1","port":538},"addr":"/ip4/127.0.0.1/tcp/538","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":538},{"peer_id":"QmVshsmSHSjk84tcac1wdncMN4hcSXpM5LSQvYkjig3YYS","ip_port":{"ip":"127.0.0.1","port":773},"addr":"/ip4/127.0.0.1/tcp/773","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":773},{"peer_id":"QmbPjmp1rQ1M4G533YPs6CvB5aP6suH92qnhJ6eA1QJJC7","ip_port":{"ip":"127.0.0.1","port":156},"addr":"/ip4/127.0.0.1/tcp/156","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":156},{"peer_id":"Qmf3xG6wJuhXP1QQQMtwAzvz5oCsMtLkyZAJEVe9oW6hae","ip_port":{"ip":"127.0.0.1","port":217},"addr":"/ip4/127.0.0.1/tcp/217","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":217},{"peer_id":"QmdAkE4i1V7ikhnKenYMQ21955q58i9b7XnUcKVRs9TEtP","ip_port":{"ip":"127.0.0.1","port":594},"addr":"/ip4/127.0.0.1/tcp/594","score":60,"last_connected_at_ms":0,"last_tried_at_ms":0,"attempts_count":0,"random_id_pos":594},{"peer_id":"QmawsN1dHNMMx1sDWXdp2r8H8rXanu"#;
        writeln!(file, "{}", truncated_json).unwrap();
        file.sync_all().unwrap();
    }
    // write a truncated json with 3 ban records to tmp file
    {
        let tmp_ban_file_path = dir.path().join("tmp/ban_list.db");
        let mut file = File::create(tmp_ban_file_path).unwrap();
        let truncated_json = r#"[{"address":"192.168.0.2/32","ban_until":31061427677740,"ban_reason":"test","created_at":1612678877739},{"address":"192.168.0.3/32","ban_until":472792659688893,"ban_reason":"test","created_at":1612678888636},{"address":"192.168.0.4/32","ban_until":472792659688893,"ban_reason"#;
        writeln!(file, "{}", truncated_json).unwrap();
        file.sync_all().unwrap();
    }

    // dump with 3 peers and 1 ban list
    let mut peer_store = PeerStore::default();
    let addr_manager = peer_store.mut_addr_manager();
    for i in 0..3 {
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", i).parse().unwrap();
        addr_manager.add(AddrInfo::new(
            PeerId::random(),
            addr.extract_ip_addr().unwrap(),
            addr,
            0,
            60,
        ));
    }
    let ban_list = peer_store.mut_ban_list();
    let now_ms = faketime::unix_time_as_millis();
    ban_list.ban(BannedAddr {
        address: multiaddr_to_ip_network(&"/ip4/127.0.0.1/tcp/42".parse().unwrap()).unwrap(),
        ban_until: now_ms + 10_000,
        ban_reason: "test".into(),
        created_at: now_ms,
    });
    peer_store.dump_to_dir(dir.as_ref()).unwrap();

    // reload from dumped data should be OK
    let peer_store = PeerStore::load_from_dir_or_default(dir.as_ref());
    assert_eq!(1, peer_store.ban_list().count());
    assert_eq!(3, peer_store.addr_manager().count());
}
