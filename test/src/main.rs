use ckb_test::node::Node;
use tempfile::tempdir;

fn main() {
    let node = Node::new(
        "../target/release/ckb",
        tempdir().unwrap().path().to_str().unwrap(),
        9000,
        9001,
    );
    node.start();
}
