mod addr_manager;
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
