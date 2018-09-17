use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::super::ProtocolEvent;
use super::protocol::TestProtocol;
use clap;
use network::Config;
use network::NetworkConfiguration;
use network::NetworkService;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use tempdir::TempDir;

pub fn run(matches: &clap::ArgMatches) {
    let boot_node_count = matches
        .value_of("boot-nodes")
        .map(|s| s.parse().unwrap())
        .unwrap_or(1);
    let other_node_count = matches
        .value_of("other-nodes")
        .map(|s| s.parse().unwrap())
        .unwrap_or(3);
    let init_wait = matches
        .value_of("init-wait")
        .map(|s| s.parse().unwrap())
        .unwrap_or(300);
    let timer = matches
        .value_of("timer")
        .map(|s| s.parse().unwrap())
        .unwrap_or(100);
    let send_msgs = matches
        .value_of("send-msgs")
        .map(|s| s.parse().unwrap())
        .unwrap_or(60);
    let keep_connect = matches
        .value_of("keep-connect")
        .map(|s| s.parse().unwrap())
        .unwrap_or(100);

    info!("Starting network: boot-nodes: {}, other-nodes: {}, init-wait: {}ms, timer: {}ms, send-msgs: {}, keep-connect: {}",
          boot_node_count,
          other_node_count,
          init_wait,
          timer,
          send_msgs,
          keep_connect,
    );
    let mut boot_nodes = (0..boot_node_count)
        .map(|id| TestNode::new(id, None, vec![], timer, send_msgs))
        .collect::<Vec<_>>();
    let boot_node_urls = boot_nodes
        .iter()
        .map(|node| node.external_url.clone())
        .collect::<Vec<_>>();
    let mut other_nodes = (boot_node_count..(boot_node_count + other_node_count))
        .map(|id| {
            thread::sleep(Duration::from_millis(init_wait));
            TestNode::new(id, None, boot_node_urls.clone(), timer, send_msgs)
        }).collect::<Vec<_>>();
    info!("Netwowrk started!");
    let mut nodes: HashMap<u32, &mut TestNode> = boot_nodes
        .iter_mut()
        .chain(other_nodes.iter_mut())
        .map(|node| (node.id, node))
        .collect();

    let begin_time = Instant::now();
    loop {
        for node in nodes.values_mut() {
            let events = node.protocol.events.read();
            while let Some((_timestamp, event)) = events.get(node.event_index) {
                match event {
                    ProtocolEvent::Read(_, _, _) => {}
                    ProtocolEvent::Timeout(_) => {}
                    _ => info!("[network.{:02}.event]: {:?}", node.id, event),
                }
                node.event_index += 1;
            }
        }

        thread::sleep(Duration::from_millis(50));

        if begin_time.elapsed() >= Duration::from_secs(keep_connect - 2) {
            for node in nodes.values() {
                *node.protocol.stop.write() = true;
            }
        }
        if begin_time.elapsed() >= Duration::from_secs(keep_connect) {
            info!(
                ">>> Successfully Keep network alive for {} seconds!",
                keep_connect
            );
            break;
        }
    }

    let mut total_read = 0;
    let mut total_send = 0;
    for (id, node) in &nodes {
        let events = node.protocol.events.read();
        let mut events_initialize = Vec::new();
        let mut events_read = Vec::new();
        let mut events_connected = Vec::new();
        let mut events_disconnected = Vec::new();
        let mut events_timeout = Vec::new();
        for (_timestamp, event) in events.iter() {
            match event {
                ProtocolEvent::Initialize => events_initialize.push(event),
                ProtocolEvent::Read(_, _, _) => events_read.push(event),
                ProtocolEvent::Connected(_, s) => {
                    events_connected.push(s.remote_address.split('/').last().unwrap());
                }
                ProtocolEvent::Disconnected(_, _) => events_disconnected.push(event),
                ProtocolEvent::Timeout(_) => events_timeout.push(event),
            }
        }
        let send_count = *node.protocol.count.read();
        total_read += events_read.len();
        total_send += send_count;
        info!("Node-{:02}-{}: initialize: {}, read:{}, send: {}, disconnected: {}, timeout: {}, connected: {:?}",
              id,
              node.local_port,
              events_initialize.len(),
              events_read.len(),
              send_count,
              events_disconnected.len(),
              events_timeout.len(),
              events_connected,
        );
    }
    info!("total_read={}, total_send={}", total_read, total_send);
}

#[allow(dead_code)]
struct TestNode {
    id: u32,
    dir: TempDir,
    port: Option<u16>,
    network: NetworkService,
    external_url: String,
    local_port: String,
    protocol: Arc<TestProtocol>,
    event_index: usize,
}

impl TestNode {
    fn new(
        id: u32,
        port: Option<u16>,
        boot_nodes: Vec<String>,
        timer: u64,
        send_msgs: u32,
    ) -> TestNode {
        let addr = format!("/ip4/127.0.0.1/tcp/{}", port.unwrap_or(0))
            .parse()
            .unwrap();
        let dir = TempDir::new(&format!("test-network-many-nodes-{}", id)).unwrap();
        let protocol = Arc::new(TestProtocol {
            events: RwLock::new(Vec::new()),
            peers: RwLock::new(HashSet::new()),
            count: RwLock::new(0),
            stop: RwLock::new(false),
            timer,
            send_msgs,
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
        let external_url = network.external_url().unwrap();
        let local_port = external_url.split('/').nth(4).unwrap().to_string();
        TestNode {
            id,
            dir,
            port,
            network,
            external_url,
            local_port,
            protocol,
            event_index: 0,
        }
    }
}
