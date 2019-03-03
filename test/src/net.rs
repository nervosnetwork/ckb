use crate::Node;
use tempfile::tempdir;

pub struct Net {
    pub nodes: Vec<Node>,
}

impl Net {
    pub fn new(binary: &str, num_nodes: usize, start_port: u16) -> Self {
        let nodes: Vec<Node> = (0..num_nodes)
            .map(|n| {
                Node::new(
                    binary,
                    tempdir().unwrap().path().to_str().unwrap(),
                    start_port + (n * 2) as u16,
                    start_port + (n * 2 + 1) as u16,
                )
            })
            .collect();

        Self { nodes }
    }
}
