#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_network::{
    multiaddr::Multiaddr, peer_store::types::BannedAddr, peer_store::PeerStore, Flags, PeerId,
};
use ckb_network_fuzz::BufManager;

fn new_multi_addr(data: &mut BufManager) -> (Multiaddr, Flags) {
    let flags = data.get();
    let addr_flag = data.get::<u8>();

    let mut addr_str = if addr_flag & 0b1 != 1 {
        let buf = data.get_buf(16);
        format!(
            "/ip6/{}",
            std::net::Ipv6Addr::from(u128::from_le_bytes(buf.try_into().unwrap()))
        )
    } else {
        format!("/ip4/{}", data.get::<std::net::Ipv4Addr>())
    };

    addr_str += &format!("/tcp/{}", data.get::<u16>());

    addr_str += &format!("/p2p/{}", data.get::<ckb_network::PeerId>().to_base58());

    (addr_str.parse().unwrap(), flags)
}

fn add_ban_addr(data: &mut BufManager, peer_store: &mut PeerStore) {
    let num = data.get::<u8>() as usize;
    for _ in 0..num {
        let flags = data.get::<u8>();

        let network = if flags & 0b1 == 1 {
            data.get::<ipnetwork::Ipv4Network>().into()
        } else {
            data.get::<ipnetwork::Ipv6Network>().into()
        };

        let ban_addr = BannedAddr {
            address: network,
            ban_until: data.get(),
            created_at: data.get(),
            ban_reason: String::new(),
        };
        peer_store.mut_ban_list().ban(ban_addr);
    }
}

fn add_basic_addr(data: &mut BufManager, peer_store: &mut PeerStore) {
    let flags = data.get::<u32>();
    if flags & 0b1 == 0 {
        return;
    }

    if (flags >> 1) & 0b1 == 1 {
        add_ban_addr(data, peer_store);
    }

    let basic_num = data.get::<u32>();

    let num = basic_num % 16 + (16384) - 8; // ±8

    for i in 0..num {
        let addr = format!(
            "/ip4/{}/tcp/43/p2p/{}",
            std::net::Ipv4Addr::from(i),
            PeerId::random().to_base58()
        )
        .parse()
        .unwrap();
        let _ = peer_store.add_addr_fuzz(addr, Flags::all(), data.get(), data.get());
    }
}

fuzz_target!(|data: &[u8]| {
    let mut data = BufManager::new(data);

    let mut peer_store: PeerStore = Default::default();

    // basic addr:
    add_basic_addr(&mut data, &mut peer_store);

    let fetch_count = data.get::<u16>() as usize;
    let fetch_flag = data.get();

    while !data.is_end() {
        let (addr, flag) = new_multi_addr(&mut data);
        let last_connected_time = data.get();
        let attempts_count = data.get::<u32>();
        let _res = peer_store.add_addr_fuzz(addr, flag, last_connected_time, attempts_count);
        // _res.expect("msg");
    }

    let ret = peer_store.fetch_random_addrs(fetch_count, fetch_flag);
    assert!(ret.len() <= fetch_count);
});
