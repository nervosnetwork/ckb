use std::convert::From;
use std::sync::Arc;
use std::thread;
use std::time;

use super::super::ProtocolEvent;
use super::protocol::TestProtocol;
use clap;
use crossbeam_channel as channel;
use network::Config;
use network::NetworkConfiguration;
use network::NetworkService;
use tempdir::TempDir;

pub fn run(_matches: &clap::ArgMatches) {
    info!("Starting network");

    let addr1 = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
    let network1_dir = TempDir::new("test-ckb-network-1").unwrap();
    let config1 = Config {
        listen_addresses: vec![addr1],
        net_config_path: Some(network1_dir.path().to_string_lossy().into_owned()),
        secret_file: Some("secret".to_string()),
        nodes_file: Some("nodes.json".to_string()),
        boot_nodes: vec![],
        reserved_nodes: Vec::new(),
        non_reserved_mode: None,
        min_peers: 4,
        max_peers: 8,
    };
    let (network1, events1) = start_network(config1);
    let network1_url = network1.external_url().unwrap();
    info!("network1.external_url: {:?}", network1_url);

    let addr2 = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
    let network2_dir = TempDir::new("test-ckb-network-2").unwrap();
    let config2 = Config {
        listen_addresses: vec![addr2],
        net_config_path: Some(network2_dir.path().to_string_lossy().into_owned()),
        secret_file: Some("secret".to_string()),
        nodes_file: Some("nodes.json".to_string()),
        boot_nodes: vec![network1_url],
        reserved_nodes: Vec::new(),
        non_reserved_mode: None,
        min_peers: 4,
        max_peers: 8,
    };
    let (network2, events2) = start_network(config2);
    info!("network2.external_url: {:?}", network2.external_url());
    info!("Netwowrk started!");

    let mut connected1 = false;
    let mut connected2 = false;
    let mut adjusted_time = false;
    let mut begin_time = time::Instant::now();
    loop {
        select! {
            recv(events1, event) => {
                let event = event.unwrap();
                info!("[network.1.event]: {:?}", event);
                match event {
                    ProtocolEvent::Connected(_, _) => {
                        connected1 = true;
                    }
                    ProtocolEvent::Disconnected(_, _) => {
                        panic!("Network 2 disconnected from network 1");
                    }
                    _ => {}
                }
            },
            recv(events2, event) => {
                let event = event.unwrap();
                info!("[network.2.event]: {:?}", event);
                match event {
                    ProtocolEvent::Connected(_, _) => {
                        connected2 = true;
                    }
                    ProtocolEvent::Disconnected(_, _) => {
                        panic!("Network 1 disconnected from network 2");
                    }
                    _ => {}
                }
            },
            default => {
                thread::sleep(time::Duration::from_millis(100));
            }
        }

        if !adjusted_time && connected1 && connected2 {
            if begin_time.elapsed() > time::Duration::from_secs(10) {
                panic!("Use more than 10 seconds to connect each other");
            } else {
                info!("All peer connected");
                begin_time = time::Instant::now();
                adjusted_time = true;
            }
        }
        let keep_connect = 45;
        if begin_time.elapsed() >= time::Duration::from_secs(keep_connect) {
            if !connected1 || !connected2 {
                panic!("Still not connected after {} socneds", keep_connect);
            }
            info!(
                "Successfully Keep network alive for {} seconds!",
                keep_connect
            );
            break;
        }
    }
}

fn start_network(config: Config) -> (Arc<NetworkService>, channel::Receiver<ProtocolEvent>) {
    let network_config = NetworkConfiguration::from(config);
    let (sender, receiver) = channel::unbounded();
    let test_protocol = Arc::new(TestProtocol { events: sender });
    let protocols = vec![(Arc::clone(&test_protocol) as Arc<_>, *b"tst", &[(1, 1)][..])];
    (
        Arc::new(NetworkService::new(network_config, protocols).expect("Create network")),
        receiver,
    )
}
