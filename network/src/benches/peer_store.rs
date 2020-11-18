#![allow(missing_docs)]
#[macro_use]
extern crate criterion;
extern crate ckb_network;
extern crate ckb_util;

use ckb_network::{multiaddr::Multiaddr, peer_store::PeerStore, PeerId};
use criterion::Criterion;

fn insert_benchmark(c: &mut Criterion) {
    c.bench_function_over_inputs(
        "Peer store insert addrs",
        |b, &&size| {
            let peer_ids = (0..size).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/255.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
            b.iter(|| {
                let mut peer_store = PeerStore::default();
                for peer_id in peer_ids.clone() {
                    peer_store.add_addr(peer_id, addr.clone()).unwrap();
                }
            });
        },
        &[10_000, 20_000],
    );
}

fn get_random_benchmark(c: &mut Criterion) {
    c.bench_function_over_inputs(
        "Peer store get random addrs",
        |b, &&size| {
            let peer_ids = (0..size).map(|_| PeerId::random()).collect::<Vec<_>>();
            let addr = "/ip4/255.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
            let mut peer_store = PeerStore::default();
            for peer_id in peer_ids {
                peer_store.add_addr(peer_id, addr.clone()).unwrap();
            }
            b.iter(|| {
                peer_store.fetch_random_addrs(size);
            });
        },
        &[10_000, 20_000],
    );
}

criterion_group!(benches, insert_benchmark, get_random_benchmark);
criterion_main!(benches);
