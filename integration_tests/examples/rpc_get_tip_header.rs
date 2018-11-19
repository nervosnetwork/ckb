extern crate ckb_test_harness;

use ckb_test_harness::rt::Future;
use ckb_test_harness::TestHarness;
use std::thread;

fn main() {
    let mut harness = TestHarness::new(2);

    harness.start();

    thread::sleep(::std::time::Duration::from_secs(10));

    let header = harness.nodes[0].rpc.get_tip_header().map_err(|_| ()).wait();

    println!("header {:?}", header);
}
