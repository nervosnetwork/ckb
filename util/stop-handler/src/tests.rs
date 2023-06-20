use crate::{
    broadcast_exit_signals, new_crossbeam_exit_rx, new_tokio_exit_rx, register_thread,
    wait_all_ckb_services_exit,
};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_channel::select;
use rand::Rng;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn send_ctrlc_later(duration: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        // send SIGINT to myself
        unsafe {
            libc::raise(libc::SIGINT);
            println!("[ $$ sent SIGINT to myself $$ ]");
        }
    });
}

#[derive(Default)]
struct TestStopMemo {
    spawned_threads_count: Arc<AtomicI64>,
    stopped_threads_count: Arc<AtomicI64>,

    spawned_tokio_task_count: Arc<AtomicI64>,
    stopped_tokio_task_count: Arc<AtomicI64>,
}

impl TestStopMemo {
    fn start_many_threads(&self) {
        for i in 0..rand::thread_rng().gen_range(3..7) {
            let join = std::thread::spawn({
                let stopped_threads_count = Arc::clone(&self.stopped_threads_count);
                move || {
                    let ticker = ckb_channel::tick(Duration::from_millis(500));
                    let deadline = ckb_channel::after(Duration::from_millis(
                        (rand::thread_rng().gen_range(1.0..5.0) * 1000.0) as u64,
                    ));

                    let stop = new_crossbeam_exit_rx();

                    loop {
                        select! {
                            recv(ticker) -> _ => {
                                println!("thread {} received tick signal", i);
                            },
                            recv(stop) -> _ => {
                                println!("thread {} received crossbeam exit signal", i);
                                stopped_threads_count.fetch_add(1, Ordering::SeqCst);
                                return;
                            },
                            recv(deadline) -> _ =>{
                                println!("thread {} finish its job", i);
                                stopped_threads_count.fetch_add(1, Ordering::SeqCst);
                                return
                            }
                        }
                    }
                }
            });

            self.spawned_threads_count.fetch_add(1, Ordering::SeqCst);
            register_thread(&format!("test thread {}", i), join);
        }
    }

    fn start_many_tokio_tasks(&self, handle: &Handle) {
        for i in 0..rand::thread_rng().gen_range(3..7) {
            let stop: CancellationToken = new_tokio_exit_rx();

            handle.spawn({
                let spawned_tokio_task_count = Arc::clone(&self.spawned_tokio_task_count);
                let stopped_tokio_task_count = Arc::clone(&self.stopped_tokio_task_count);
                async move {
                    spawned_tokio_task_count.fetch_add(1, Ordering::SeqCst);

                    let mut interval = tokio::time::interval(Duration::from_millis(500));

                    let duration = Duration::from_millis(
                        (rand::thread_rng().gen_range(1.0..5.0) * 1000.0) as u64,
                    );
                    let deadline = tokio::time::sleep(duration);
                    tokio::pin!(deadline);

                    loop {
                        tokio::select! {
                            _ = &mut deadline =>{
                                println!("tokio task {} finish its job", i);
                                stopped_tokio_task_count.fetch_add(1, Ordering::SeqCst);
                                break;
                            }
                            _ = interval.tick()=> {
                                println!("tokio task {} received tick signal", i);
                            },
                            _ = stop.cancelled() => {
                                println!("tokio task {} receive exit signal", i);
                                stopped_tokio_task_count.fetch_add(1, Ordering::SeqCst);
                                break
                            },
                            else => break,
                        }
                    }
                }
            });
        }
    }
}
#[test]
fn basic() {
    let (mut handle, mut stop_recv, _runtime) = new_global_runtime();

    ctrlc::set_handler(move || {
        broadcast_exit_signals();
    })
    .expect("Error setting Ctrl-C handler");

    send_ctrlc_later(Duration::from_secs(3));

    let test_memo = TestStopMemo::default();

    test_memo.start_many_threads();
    test_memo.start_many_tokio_tasks(&handle);

    handle.drop_guard();
    wait_all_ckb_services_exit();
    handle.block_on(async move {
        stop_recv.recv().await;
    });

    assert_eq!(
        test_memo.spawned_threads_count.load(Ordering::SeqCst),
        test_memo.stopped_threads_count.load(Ordering::SeqCst),
    );
    assert_eq!(
        test_memo.spawned_tokio_task_count.load(Ordering::SeqCst),
        test_memo.stopped_tokio_task_count.load(Ordering::SeqCst),
    );
}
