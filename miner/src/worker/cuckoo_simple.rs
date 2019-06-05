use super::{Worker, WorkerMessage};
use byteorder::{ByteOrder, LittleEndian};
use ckb_core::header::Seal;
use ckb_logger::{debug, error};
use ckb_pow::{pow_message, Cuckoo, CuckooSip};
use crossbeam_channel::{Receiver, Sender};
use indicatif::ProgressBar;
use numext_fixed_hash::H256;
use rand::random;
use std::thread;
use std::time::{Duration, SystemTime};

pub struct CuckooSimple {
    start: bool,
    pow_hash: Option<H256>,
    cuckoo: Cuckoo,
    seal_tx: Sender<(H256, Seal)>,
    worker_rx: Receiver<WorkerMessage>,
    seal_candidates_found: u64,
}

impl CuckooSimple {
    pub fn new(
        cuckoo: Cuckoo,
        seal_tx: Sender<(H256, Seal)>,
        worker_rx: Receiver<WorkerMessage>,
    ) -> Self {
        Self {
            start: true,
            pow_hash: None,
            seal_candidates_found: 0,
            cuckoo,
            seal_tx,
            worker_rx,
        }
    }

    fn poll_worker_message(&mut self) {
        if let Ok(msg) = self.worker_rx.try_recv() {
            match msg {
                WorkerMessage::NewWork(pow_hash) => {
                    self.pow_hash = Some(pow_hash);
                }
                WorkerMessage::Stop => {
                    self.start = false;
                }
                WorkerMessage::Start => {
                    self.start = true;
                }
            }
        }
    }

    fn solve(&mut self, pow_hash: &H256, nonce: u64) {
        debug!("solve, pow_hash {:x}, nonce {:?}", pow_hash, nonce);
        let mut graph = vec![0; (self.cuckoo.max_edge << 1) as usize].into_boxed_slice();
        let keys = CuckooSip::message_to_keys(&pow_message(pow_hash, nonce));
        let hasher = CuckooSip::new(keys[0], keys[1], keys[2], keys[3]);

        for e in 0..self.cuckoo.max_edge {
            let (u, v) = {
                let edge = hasher.edge(e as u32, self.cuckoo.edge_mask);
                (edge.0 << 1, (edge.1 << 1) + 1)
            };
            if u == 0 {
                continue;
            }
            let path_u = path(&graph, u);
            let path_v = path(&graph, v);
            if path_u.last().is_some() && (path_u.last() == path_v.last()) {
                let common = path_u
                    .iter()
                    .rev()
                    .zip(path_v.iter().rev())
                    .take_while(|(u, v)| u == v)
                    .count();
                if (path_u.len() - common) + (path_v.len() - common) + 1 == self.cuckoo.cycle_length
                {
                    let mut cycle: Vec<_> = {
                        let list: Vec<_> = path_u
                            .iter()
                            .take(path_u.len() - common + 1)
                            .chain(path_v.iter().rev().skip(common))
                            .chain(::std::iter::once(&u))
                            .cloned()
                            .collect();
                        list.windows(2).map(|edge| (edge[0], edge[1])).collect()
                    };
                    let mut result = Vec::with_capacity(self.cuckoo.cycle_length);
                    for n in 0..self.cuckoo.max_edge {
                        let cur_edge = {
                            let edge = hasher.edge(n as u32, self.cuckoo.edge_mask);
                            (edge.0 << 1, (edge.1 << 1) + 1)
                        };
                        for i in 0..cycle.len() {
                            let cycle_edge = cycle[i];
                            if cycle_edge == cur_edge || (cycle_edge.1, cycle_edge.0) == cur_edge {
                                result.push(n as u32);
                                cycle.remove(i);
                                break;
                            }
                        }
                    }

                    let mut proof_u8 = vec![0u8; self.cuckoo.cycle_length << 2];
                    LittleEndian::write_u32_into(&result, &mut proof_u8);
                    let seal = Seal::new(nonce, proof_u8.into());
                    debug!(
                        "send new found seal, pow_hash {:x}, seal {:?}",
                        pow_hash, seal
                    );
                    if let Err(err) = self.seal_tx.send((pow_hash.clone(), seal)) {
                        error!("seal_tx send error {:?}", err);
                    }
                    self.seal_candidates_found += 1;
                }
            } else if path_u.len() < path_v.len() {
                for edge in path_u.windows(2) {
                    graph[edge[1] as usize] = edge[0];
                }
                graph[u as usize] = v;
            } else {
                for edge in path_v.windows(2) {
                    graph[edge[1] as usize] = edge[0];
                }
                graph[v as usize] = u;
            }
        }
    }
}

fn path(graph: &[u64], start: u64) -> Vec<u64> {
    let mut node = start;
    let mut path = vec![start];
    loop {
        node = graph[node as usize];
        if node != 0 {
            path.push(node);
        } else {
            break;
        }
    }
    path
}

impl Worker for CuckooSimple {
    fn run(&mut self, progress_bar: ProgressBar) {
        loop {
            self.poll_worker_message();
            if self.start {
                if let Some(pow_hash) = self.pow_hash.clone() {
                    let start = SystemTime::now();
                    self.solve(&pow_hash, random());
                    let elapsed = SystemTime::now().duration_since(start).unwrap();
                    let elapsed_nanos: f64 = (elapsed.as_secs() * 1_000_000_000
                        + u64::from(elapsed.subsec_nanos()))
                        as f64
                        / 1_000_000_000.0;
                    progress_bar.set_message(&format!(
                        "Graphs per second: {:>10.3} / Total seal candidates found: {:>10}",
                        1.0 / elapsed_nanos,
                        self.seal_candidates_found,
                    ));
                    progress_bar.inc(1);
                }
            } else {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ckb_pow::{CuckooEngine, PowEngine};
    use crossbeam_channel::unbounded;
    use proptest::prelude::*;

    fn _cuckoo_solve(pow_hash: &H256, nonce: u64) -> Result<(), TestCaseError> {
        let (seal_tx, seal_rx) = unbounded();
        let (_worker_tx, worker_rx) = unbounded();
        let cuckoo = Cuckoo::new(6, 8);
        let mut worker = CuckooSimple::new(cuckoo.clone(), seal_tx, worker_rx);
        worker.solve(pow_hash, nonce);
        let engine = CuckooEngine { cuckoo };
        while let Ok((pow_hash, seal)) = seal_rx.try_recv() {
            let (nonce, proof) = seal.destruct();
            let message = pow_message(&pow_hash, nonce);
            prop_assert!(engine.verify(0, &message, &proof));
        }

        Ok(())
    }

    proptest! {
        #[test]
        fn cuckoo_solve(h256 in prop::array::uniform32(0u8..), nonce in any::<u64>()) {
            _cuckoo_solve(&H256::from_slice(&h256).unwrap(), nonce)?;
        }
    }
}
