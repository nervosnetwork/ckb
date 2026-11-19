use super::*;
use ckb_channel::{Sender, bounded};
use ckb_shared::SharedBuilder;
use ckb_tx_pool::internal_test_support::verification_commands;
use ckb_types::{core::BlockBuilder, prelude::*};
use std::{
    sync::Mutex,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

struct ConsumerThread {
    stop: Sender<()>,
    task: Option<JoinHandle<()>>,
}

impl Drop for ConsumerThread {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(task) = self.task.take() {
            task.join().expect("the verification consumer must join");
        }
    }
}

#[test]
fn queued_blocks_keep_one_pause_across_callback_unwind_and_restore_when_idle() {
    let (shared, mut package) = SharedBuilder::with_temp_db().build().unwrap();
    let controller = shared.tx_pool_controller().clone();
    let mut commands = verification_commands(&controller);
    let resumed = commands.borrow_and_update().clone();
    let commands = Arc::new(Mutex::new(commands));
    let (blocks, block_rx) = bounded(2);
    let (truncate, truncate_rx) = bounded(1);
    let (stop, stop_rx) = bounded(1);
    let (observations, received) = bounded(2);
    let parent = [42u8; 32].pack();
    shared.insert_block_status(parent.clone(), BlockStatus::BLOCK_INVALID);

    // Both requests are queued before the real consumer starts. Their invalid
    // parent exercises rejection without depending on expensive block scripts.
    for index in 0..2 {
        let block = BlockBuilder::default()
            .number(1)
            .epoch(
                shared
                    .consensus()
                    .genesis_epoch_ext()
                    .number_with_fraction(1),
            )
            .nonce(index)
            .parent_hash(parent.clone())
            .build();
        let commands = Arc::clone(&commands);
        let observations = observations.clone();
        let resumed = resumed.clone();
        blocks
            .send(UnverifiedBlock {
                block: Arc::new(block),
                parent_header: shared.consensus().genesis_block().header(),
                switch: Some(Switch::DISABLE_ALL),
                verify_callback: Some(Box::new(move |result| {
                    let mut commands = commands.lock().unwrap();
                    let changed = commands.has_changed().unwrap();
                    let paused = *commands.borrow_and_update() != resumed;
                    observations
                        .send((result.is_err(), paused, changed))
                        .unwrap();
                    drop(commands);
                    if index == 0 {
                        panic!("exercise the consumer's callback-unwind recovery");
                    }
                })),
            })
            .unwrap();
    }
    let builder = package.take_chain_services_builder();
    let consumer = ConsumeUnverifiedBlocks::new(
        shared,
        block_rx,
        truncate_rx,
        builder.proposal_table,
        Arc::new(DashSet::new()),
        stop_rx,
    );
    let consumer_thread = ConsumerThread {
        stop,
        task: Some(thread::spawn(move || consumer.start())),
    };
    assert_eq!(received.recv_timeout(WAIT).unwrap(), (true, true, true));
    assert_eq!(received.recv_timeout(WAIT).unwrap(), (true, true, false));

    // Observe the command transition before sending Stop: resumption must come
    // from reaching idle, not from terminating the test's consumer.
    let deadline = Instant::now() + WAIT;
    loop {
        let commands = commands.lock().unwrap();
        if commands.has_changed().unwrap() {
            assert_eq!(*commands.borrow(), resumed);
            break;
        }
        assert!(Instant::now() < deadline, "idle must restore verification");
        drop(commands);
        thread::yield_now();
    }
    drop(consumer_thread);
    drop(truncate);
}

#[test]
fn verification_pause_restores_on_unwind() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let controller = shared.tx_pool_controller();
    let mut commands = verification_commands(controller);
    let resumed = commands.borrow_and_update().clone();
    let pause = VerificationPause::new(controller);
    assert_ne!(*commands.borrow(), resumed);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _pause = pause;
        panic!("exercise pause ownership during unwind");
    }));
    assert!(result.is_err());
    assert_eq!(*commands.borrow(), resumed);
}
