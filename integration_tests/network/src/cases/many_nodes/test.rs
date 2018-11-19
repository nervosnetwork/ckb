use clap;
use std::sync::Arc;
use std::thread;
use std::time;

use super::super::ProtocolEvent;
use super::protocol::TestProtocol;
use network::Config;
use network::NetworkConfiguration;
use network::NetworkService;
use parking_lot::RwLock;
use tempdir::TempDir;

pub fn run(matches: &clap::ArgMatches) {
    let boot_node_count = matches
        .value_of("boot-nodes")
        .map(|s| s.parse().unwrap())
        .unwrap_or(10);
    let other_node_count = matches
        .value_of("other-nodes")
        .map(|s| s.parse().unwrap())
        .unwrap_or(15);
    let init_wait = matches
        .value_of("init-wait")
        .map(|s| s.parse().unwrap())
        .unwrap_or(1000);
    let should_connect = matches
        .value_of("should-connect")
        .map(|s| s.parse().unwrap())
        .unwrap_or(60);
    let keep_connect = matches
        .value_of("keep-connect")
        .map(|s| s.parse().unwrap())
        .unwrap_or(120);

    info!("Starting network: boot-nodes: {}, other-nodes: {}, init-wait: {}ms, should-connect: {}, keep-connect: {}",
          boot_node_count,
          other_node_count,
          init_wait,
          should_connect,
          keep_connect,
    );
    let mut boot_nodes = (0..boot_node_count)
        .map(|id| {
            thread::sleep(time::Duration::from_millis(init_wait));
            TestNode::new(id, None, vec![])
        }).collect::<Vec<_>>();
    let boot_node_urls = boot_nodes
        .iter()
        .map(|node| node.network.external_url().unwrap())
        .collect::<Vec<_>>();
    let mut other_nodes = (boot_node_count..(boot_node_count + other_node_count))
        .map(|id| {
            thread::sleep(time::Duration::from_millis(init_wait));
            TestNode::new(id, None, boot_node_urls.clone())
        }).collect::<Vec<_>>();
    info!("Netwowrk started!");

    // let mut adjusted_time = false;
    let begin_time = time::Instant::now();
    loop {
        for node in boot_nodes.iter_mut().chain(other_nodes.iter_mut()) {
            let events = node.protocol.events.read();
            while let Some((_timestamp, event)) = events.get(node.event_index) {
                info!("[network.{:02}.event]: {:?}", node.id, event);
                match event {
                    ProtocolEvent::Connected(_, _) => {
                        node.connected = true;
                    }
                    ProtocolEvent::Disconnected(_, _) => {
                        // panic!("Some network disconnected from network {}", node.id);
                    }
                    _ => {}
                }
                node.event_index += 1;
            }
            if !node.connected && begin_time.elapsed() > time::Duration::from_secs(should_connect) {
                error!("Node {} have not been connected after 60 seconds", node.id);
            }
        }

        thread::sleep(time::Duration::from_millis(50));

        if begin_time.elapsed() >= time::Duration::from_secs(keep_connect) {
            info!(
                ">>> Successfully Keep network alive for {} seconds!",
                keep_connect
            );
            break;
        }
    }
}

#[allow(dead_code)]
struct TestNode {
    id: u32,
    dir: TempDir,
    port: Option<u16>,
    network: NetworkService,
    protocol: Arc<TestProtocol>,
    connected: bool,
    event_index: usize,
}

impl TestNode {
    fn new(id: u32, port: Option<u16>, boot_nodes: Vec<String>) -> TestNode {
        let addr = format!("/ip4/127.0.0.1/tcp/{}", port.unwrap_or(0))
            .parse()
            .unwrap();
        let dir = TempDir::new(&format!("test-network-many-nodes-{}", id)).unwrap();
        let protocol = Arc::new(TestProtocol {
            events: RwLock::new(Vec::new()),
        });
        let protocols = vec![(Arc::clone(&protocol) as Arc<_>, *b"tst", &[(1, 1)][..])];
        let config = Config {
            listen_addresses: vec![addr],
            net_config_path: Some(dir.path().to_string_lossy().into_owned()),
            secret_file: Some("secret".to_string()),
            nodes_file: Some("nodes.json".to_string()),
            boot_nodes,
            reserved_nodes: Vec::new(),
            non_reserved_mode: None,
            min_peers: 8,
            // FIXME: When it is too small, connected node will be disconnected,
            //   don't know why yet.
            max_peers: 16,
        };
        let network_config = NetworkConfiguration::from(config);
        let network = NetworkService::new(network_config, protocols).expect("Create network");
        TestNode {
            id,
            dir,
            port,
            network,
            protocol,
            connected: false,
            event_index: 0,
        }
    }
}
