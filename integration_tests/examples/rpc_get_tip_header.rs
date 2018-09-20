extern crate ckb_test_harness;

use ckb_test_harness::rt::Future;
use ckb_test_harness::TestHarness;
use std::thread;

fn main() {
    let mut harness = TestHarness::new(2);

    harness.start();

    thread::sleep(::std::time::Duration::from_secs(10));

    let header = harness.nodes[0].rpc.get_tip_header().wait();

    println!("tip header 0 {:?}", header);

    harness.nodes[0].rpc.submit_pow_solution(1).wait().unwrap();

    let header = harness.nodes[0].rpc.get_tip_header().wait();

    println!("tip header 1 {:?}", header);

    drop(harness);
}
