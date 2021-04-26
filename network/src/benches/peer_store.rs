#![allow(missing_docs)]
#[macro_use]
extern crate criterion;
extern crate ckb_network;
extern crate ckb_util;

use ckb_network::{multiaddr::Multiaddr, peer_store::PeerStore, PeerId};
use criterion::{BatchSize, BenchmarkId, Criterion};

const SIZES: &[usize] = &[10_000, 20_000];

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("peer_store");

    for size in SIZES.iter() {
        group.bench_with_input(BenchmarkId::new("add_addr", size), size, |b, i| {
            b.iter_batched(
                || {
                    let peer_ids = (0..*i).map(|_| PeerId::random()).collect::<Vec<_>>();
                    let addr = "/ip4/255.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
                    (peer_ids, addr)
                },
                |(peer_ids, addr)| {
                    let mut peer_store = PeerStore::default();
                    for peer_id in peer_ids.clone() {
                        peer_store.add_addr(peer_id, addr.clone()).unwrap();
                    }
                },
                BatchSize::PerIteration,
            )
        });
    }

    for size in SIZES.iter() {
        group.bench_with_input(
            BenchmarkId::new("fetch_random_addrs", size),
            size,
            |b, i| {
                b.iter_batched(
                    || {
                        let peer_ids = (0..*i).map(|_| PeerId::random()).collect::<Vec<_>>();
                        let addr = "/ip4/255.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
                        let mut peer_store = PeerStore::default();
                        for peer_id in peer_ids {
                            peer_store.add_addr(peer_id, addr.clone()).unwrap();
                        }
                        peer_store
                    },
                    |mut peer_store| {
                        peer_store.fetch_random_addrs(*i);
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
