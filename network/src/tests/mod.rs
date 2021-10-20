mod addr_manager;
mod compress;
mod peer_registry;
mod peer_store;
mod peer_store_db;

fn random_addr() -> crate::multiaddr::Multiaddr {
    format!(
        "/ip4/127.0.0.1/tcp/42/p2p/{}",
        crate::PeerId::random().to_base58()
    )
    .parse()
    .unwrap()
}

fn random_addr_v6() -> crate::multiaddr::Multiaddr {
    let addr = std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc00a, 0x2ff);
    let mut multi_addr: crate::multiaddr::Multiaddr = addr.into();

    multi_addr.push(crate::multiaddr::Protocol::Tcp(43));
    multi_addr.push(crate::multiaddr::Protocol::P2P(
        crate::PeerId::random().to_base58().into_bytes().into(),
    ));
    multi_addr
}
