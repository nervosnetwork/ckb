#[macro_use]
extern crate criterion;
extern crate ckb_network;
extern crate ckb_util;

use ckb_network::{
    multiaddr::ToMultiaddr,
    peer_store::{PeerStore, SqlitePeerStore},
    PeerId, SessionType,
};
use criterion::Criterion;
use std::rc::Rc;

fn insert_peer_info_benchmark(c: &mut Criterion) {
    c.bench_function("insert 100 peer_info", |b| {
        b.iter({
            let peer_store = SqlitePeerStore::memory("bench_db_insert_100_peer_info".to_string())
                .expect("memory");
            let peer_ids = (0..100).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                }
            }
        })
    });
    c.bench_function("insert 1000 peer_info", |b| {
        b.iter({
            let peer_store = SqlitePeerStore::memory("bench_db_insert_1000_peer_info".to_string())
                .expect("memory");
            let peer_ids = (0..1000).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
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
            let peer_store = SqlitePeerStore::temp().expect("temp");
            let peer_ids = (0..100).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
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
        let peer_store: Rc<dyn PeerStore> =
            Rc::new(SqlitePeerStore::memory("bench_db_random_order".to_string()).expect("memory"));
        let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
        {
            for _ in 0..8000 {
                let peer_id = PeerId::random();
                peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                let _ = peer_store.add_discovered_addr(&peer_id, addr.clone());
            }
        }
        c.bench_function("random order 1000 / 8000 peer_info", {
            let peer_store = Rc::clone(&peer_store);
            move |b| {
                b.iter({
                    let peer_store = Rc::clone(&peer_store);
                    move || {
                        let peer_store = Rc::clone(&peer_store);
                        let count = 1000;
                        assert_eq!(peer_store.peers_to_attempt(count).len() as u32, count);
                    }
                })
            }
        });
        c.bench_function("random order 2000 / 8000 peer_info", {
            let peer_store = Rc::clone(&peer_store);
            move |b| {
                b.iter({
                    let peer_store = Rc::clone(&peer_store);
                    move || {
                        let peer_store = Rc::clone(&peer_store);
                        let count = 2000;
                        assert_eq!(peer_store.peers_to_attempt(count).len() as u32, count);
                    }
                })
            }
        });
    }

    // filesystem benchmark
    c.bench_function(
        "random order 1000 / 8000 peer_info on filesystem",
        move |b| {
            b.iter({
                let peer_store = SqlitePeerStore::temp().expect("temp");
                let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
                for _ in 0..8000 {
                    let peer_id = PeerId::random();
                    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                    let _ = peer_store.add_discovered_addr(&peer_id, addr.clone());
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
