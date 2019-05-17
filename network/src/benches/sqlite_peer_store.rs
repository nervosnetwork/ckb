#[macro_use]
extern crate criterion;
extern crate ckb_network;
extern crate ckb_util;

use ckb_network::{
    multiaddr::Multiaddr,
    peer_store::{PeerStore, SqlitePeerStore},
    PeerId, SessionType,
};
use criterion::Criterion;

fn insert_peer_info_benchmark(c: &mut Criterion) {
    c.bench_function("insert 100 peer_info", |b| {
        b.iter({
            let mut peer_store = SqlitePeerStore::memory().expect("memory");
            let peer_ids = (0..100).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                }
            }
        })
    });
    c.bench_function("insert 1000 peer_info", |b| {
        b.iter({
            let mut peer_store = SqlitePeerStore::memory().expect("memory");
            let peer_ids = (0..1000).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                }
            }
        })
    });

    // filesystem benchmark
    c.bench_function("insert 100 peer_info on filesystem", move |b| {
        b.iter({
            let mut peer_store = SqlitePeerStore::temp().expect("temp");
            let peer_ids = (0..100).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                }
            }
        })
    });
}

fn random_order_benchmark(c: &mut Criterion) {
    {
        let mut peer_store = SqlitePeerStore::memory().expect("temp");
        let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
        {
            for _ in 0..8000 {
                let peer_id = PeerId::random();
                peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                peer_store.add_discovered_addr(&peer_id, addr.clone());
            }
        }
        c.bench_function("random order 1000 / 8000 peer_info", {
            move |b| {
                b.iter(|| {
                    let count = 1000;
                    assert_eq!(peer_store.peers_to_attempt(count).len() as u32, count);
                })
            }
        });

        let mut peer_store = SqlitePeerStore::memory().expect("temp");
        let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
        {
            for _ in 0..8000 {
                let peer_id = PeerId::random();
                peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                peer_store.add_discovered_addr(&peer_id, addr.clone());
            }
        }
        c.bench_function("random order 2000 / 8000 peer_info", {
            move |b| {
                b.iter(|| {
                    let count = 2000;
                    assert_eq!(peer_store.peers_to_attempt(count).len() as u32, count);
                })
            }
        });
    }

    // filesystem benchmark
    c.bench_function(
        "random order 1000 / 8000 peer_info on filesystem",
        move |b| {
            b.iter({
                let mut peer_store = SqlitePeerStore::temp().expect("temp");
                let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
                for _ in 0..8000 {
                    let peer_id = PeerId::random();
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                    peer_store.add_discovered_addr(&peer_id, addr.clone());
                }
                move || {
                    let count = 1000;
                    assert_eq!(peer_store.peers_to_attempt(count).len() as u32, count);
                }
            })
        },
    );
}

criterion_group!(benches, insert_peer_info_benchmark, random_order_benchmark);
criterion_main!(benches);
