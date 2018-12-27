#[macro_use]
extern crate criterion;
extern crate ckb_network;
extern crate ckb_util;
extern crate tempfile;

use ckb_network::{
    peer_store::{PeerStore, SqlitePeerStore, StorePath},
    random_peer_id, Endpoint, ToMultiaddr,
};
use ckb_util::Mutex;
use criterion::Criterion;
use std::fs;
use std::rc::Rc;
use tempfile::tempdir;

fn insert_peer_info_benchmark(c: &mut Criterion) {
    c.bench_function("insert 100 peer_info", |b| {
        b.iter({
            let mut peer_store = SqlitePeerStore::default();
            let peer_ids = (0..100)
                .map(|_| random_peer_id().unwrap())
                .collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
                }
            }
        })
    });
    c.bench_function("insert 1000 peer_info", |b| {
        b.iter({
            let mut peer_store = SqlitePeerStore::default();
            let peer_ids = (0..1000)
                .map(|_| random_peer_id().unwrap())
                .collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
                }
            }
        })
    });

    // filesystem benchmark
    let dir = tempdir().expect("temp dir");
    let file_path = dir.path().join("test.db").to_str().unwrap().to_string();
    c.bench_function("insert 100 peer_info on filesystem", move |b| {
        b.iter({
            let file_path = file_path.clone();
            let _ = fs::remove_file(file_path.clone());
            let mut peer_store = SqlitePeerStore::new(StorePath::File(file_path), 8);
            let peer_ids = (0..100)
                .map(|_| random_peer_id().unwrap())
                .collect::<Vec<_>>();
            let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
            move || {
                for peer_id in peer_ids.clone() {
                    peer_store.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
                }
            }
        })
    });
}

fn random_order_benchmark(c: &mut Criterion) {
    {
        let peer_store = Rc::new(Mutex::new(SqlitePeerStore::default()));
        let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
        {
            let mut peer_store = peer_store.lock();
            for _ in 0..8000 {
                let peer_id = random_peer_id().unwrap();
                peer_store.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
                let _ = peer_store.add_discovered_address(&peer_id, addr.clone());
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
                        assert_eq!(
                            peer_store.lock().peers_to_attempt(count).len() as u32,
                            count
                        );
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
                        assert_eq!(
                            peer_store.lock().peers_to_attempt(count).len() as u32,
                            count
                        );
                    }
                })
            }
        });
    }

    // filesystem benchmark
    let dir = tempdir().expect("temp dir");
    let file_path = dir.path().join("test.db").to_str().unwrap().to_string();
    c.bench_function(
        "random order 1000 / 8000 peer_info on filesystem",
        move |b| {
            b.iter({
                let file_path = file_path.clone();
                let _ = fs::remove_file(file_path.clone());
                let mut peer_store = SqlitePeerStore::new(StorePath::File(file_path), 8);
                let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
                for _ in 0..8000 {
                    let peer_id = random_peer_id().unwrap();
                    peer_store.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
                    let _ = peer_store.add_discovered_address(&peer_id, addr.clone());
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
