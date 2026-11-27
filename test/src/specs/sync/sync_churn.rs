use crate::node::{connect_all, make_bootnodes_for_all, waiting_for_sync_with_timeout};
use crate::util::mining::out_ibd_mode;
use crate::{Node, Spec};
use ckb_logger::info;
use rand::Rng;
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

fn select_random_node<'a, R: Rng>(rng: &mut R, nodes: &'a mut [Node]) -> &'a mut Node {
    let index = rng.gen_range(0..nodes.len());
    &mut nodes[index]
}

struct SetFlagOnDrop(Arc<AtomicBool>);

impl Drop for SetFlagOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Runs the concurrent mining and restart loops with bidirectional panic cancellation:
/// - On normal completion of `mining` (e.g. block cap reached), `restart` still finishes
///   all `num_restarts` iterations; when `restart` finishes, `mining` stops on its next check.
/// - If either thread panics, `abort` is set immediately so the sibling exits on its next
///   iteration, both threads are joined cleanly, and the chronologically first panic payload
///   is re-thrown via `resume_unwind`.
fn run_churn_loops<FM, FR>(num_restarts: usize, mut mine_step: FM, mut restart_step: FR)
where
    FM: FnMut() -> bool + Send + 'static,
    FR: FnMut(usize) + Send + 'static,
{
    let restart_done = Arc::new(AtomicBool::new(false));
    let abort = Arc::new(AtomicBool::new(false));
    let first_panic: Arc<Mutex<Option<Box<dyn Any + Send>>>> = Arc::new(Mutex::new(None));
    let thread_ctx = crate::utils::SpecThreadContext::capture();

    let mining_ctx = thread_ctx.clone();
    let mining_restart_done = Arc::clone(&restart_done);
    let mining_abort = Arc::clone(&abort);
    let mining_first_panic = Arc::clone(&first_panic);
    let mining_thread = thread::spawn(move || {
        let _ctx_guard = mining_ctx.enter();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while !mining_abort.load(Ordering::SeqCst)
                && !mining_restart_done.load(Ordering::SeqCst)
            {
                if !mine_step() {
                    break;
                }
            }
        }));
        if let Err(payload) = res {
            mining_abort.store(true, Ordering::SeqCst);
            let mut slot = mining_first_panic.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = Some(payload);
            }
        }
    });

    let restart_done_flag = Arc::clone(&restart_done);
    let restart_abort = Arc::clone(&abort);
    let restart_first_panic = Arc::clone(&first_panic);
    let restart_thread = thread::spawn(move || {
        let _ctx_guard = thread_ctx.enter();
        let _done_guard = SetFlagOnDrop(restart_done_flag);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for iter in 0..num_restarts {
                if restart_abort.load(Ordering::SeqCst) {
                    break;
                }
                restart_step(iter);
            }
        }));
        if let Err(payload) = res {
            restart_abort.store(true, Ordering::SeqCst);
            let mut slot = restart_first_panic
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = Some(payload);
            }
        }
    });

    let _ = mining_thread.join();
    let _ = restart_thread.join();
    if let Some(payload) = first_panic.lock().unwrap_or_else(|e| e.into_inner()).take() {
        std::panic::resume_unwind(payload);
    }
}

pub struct SyncChurn;

/// This test will start 5 nodes, and randomly restart 4 nodes in the middle of mining.
/// After all nodes are synced, the test is considered successful.
/// This test is used to test the robustness of the sync protocol.
/// If the sync protocol is not robust enough, the test will fail.
/// But this test is not a complete test, it can only test the robustness of the sync protocol to a certain extent.
/// Some weaknesses of this test:
/// 1. This test only consider the simple case of some nodes restarting in the middle of mining,
///    while other nodes are always mining correctly.
/// 2. This fault injection of restarting nodes is not comprehensive enough.
/// 3. Even if the test fails, we can't deterministically reproduce the same error.
///    We may need some foundationdb-like tools to deterministically reproduce the same error.
impl Spec for SyncChurn {
    crate::setup!(num_nodes: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        make_bootnodes_for_all(nodes);
        out_ibd_mode(nodes);

        #[cfg(target_os = "linux")]
        const NUM_MINED_BLOCKS: usize = 10000;
        #[cfg(target_os = "linux")]
        const NUM_RESTART: usize = 100;
        #[cfg(target_os = "linux")]
        const SYNC_TIMEOUT_SECS: u64 = 120;

        #[cfg(not(target_os = "linux"))]
        const NUM_MINED_BLOCKS: usize = 1000;
        #[cfg(not(target_os = "linux"))]
        const NUM_RESTART: usize = 20;
        #[cfg(not(target_os = "linux"))]
        const SYNC_TIMEOUT_SECS: u64 = 240;

        connect_all(nodes);
        waiting_for_sync_with_timeout(nodes, SYNC_TIMEOUT_SECS);

        let mut mining_nodes = nodes.clone();
        let mut churn_nodes = mining_nodes.split_off(2);

        run_churn_loops(
            NUM_RESTART,
            move || {
                let mut rng = rand::thread_rng();
                let mining_node = select_random_node(&mut rng, &mut mining_nodes);
                mining_node.mine(1);
                // `waiting_for_sync_with_timeout` only waits up to `SYNC_TIMEOUT_SECS`, and we
                // can sync about 200 blocks per second, so `NUM_MINED_BLOCKS` should stay within
                // that budget for each platform. Otherwise nodes may not be able to sync within
                // the configured timeout.
                if mining_node.get_tip_block_number() > NUM_MINED_BLOCKS as u64 {
                    return false;
                }
                info!(
                    "mining_node {}, tip: {}",
                    mining_node.node_id(),
                    mining_node.get_tip_block_number()
                );
                waiting_for_sync_with_timeout(&mining_nodes, SYNC_TIMEOUT_SECS);
                true
            },
            move |_| {
                let mut rng = rand::thread_rng();
                let node = select_random_node(&mut rng, &mut churn_nodes);
                info!("Restarting node {}", node.node_id());
                node.stop();
                node.start();
            },
        );

        info!("Waiting for all nodes sync");
        connect_all(nodes);
        waiting_for_sync_with_timeout(nodes, SYNC_TIMEOUT_SECS);
    }
}

#[cfg(test)]
mod tests {
    use super::run_churn_loops;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn normal_mining_cap_still_completes_all_restarts() {
        let mined = Arc::new(AtomicUsize::new(0));
        let restarted = Arc::new(AtomicUsize::new(0));
        let mined_c = Arc::clone(&mined);
        let restarted_c = Arc::clone(&restarted);

        run_churn_loops(
            20,
            move || {
                let n = mined_c.fetch_add(1, Ordering::SeqCst) + 1;
                n < 3
            },
            move |_| {
                restarted_c.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert_eq!(mined.load(Ordering::SeqCst), 3);
        assert_eq!(restarted.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn restart_panic_stops_mining_promptly_and_forwards_error() {
        let mined = Arc::new(AtomicUsize::new(0));
        let mined_c = Arc::clone(&mined);

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            run_churn_loops(
                20,
                move || {
                    let n = mined_c.fetch_add(1, Ordering::SeqCst) + 1;
                    std::thread::sleep(Duration::from_millis(5));
                    n < 1000
                },
                move |_| {
                    panic!("restart node crashed");
                },
            );
        }));

        let payload = res.expect_err("restart panic should propagate");
        assert_eq!(
            crate::extract_panic_message(payload.as_ref()),
            "restart node crashed"
        );
        assert!(
            mined.load(Ordering::SeqCst) < 50,
            "mining loop should stop promptly after restart panic, ran {} times",
            mined.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn mining_panic_stops_restarts_promptly_and_forwards_error() {
        let restarted = Arc::new(AtomicUsize::new(0));
        let restarted_c = Arc::clone(&restarted);

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            run_churn_loops(
                20,
                move || {
                    panic!("mining node crashed");
                },
                move |_| {
                    restarted_c.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                },
            );
        }));

        let payload = res.expect_err("mining panic should propagate");
        assert_eq!(
            crate::extract_panic_message(payload.as_ref()),
            "mining node crashed"
        );
        assert!(
            restarted.load(Ordering::SeqCst) < 10,
            "restart loop should stop promptly after mining panic, ran {} times",
            restarted.load(Ordering::SeqCst)
        );
    }
}
