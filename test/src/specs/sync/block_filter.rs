use crate::{Net, Node, Spec};
use ckb_hash::blake2b_256;
use ckb_logger::info;
use ckb_network::SupportProtocols;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use std::time::Duration;

const BATCH_SIZE: BlockNumber = 2000;
const CHECK_POINT_INTERVAL: BlockNumber = 2000;

pub struct GetBlockFilterCheckPoints;

impl Spec for GetBlockFilterCheckPoints {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = nodes.pop().unwrap();
        let points_num = 2;
        node.mine(CHECK_POINT_INTERVAL * points_num + 1);

        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Filter],
        );
        net.connect(&node);
        let start_number: u64 = 0;
        let request = {
            let content = packed::GetBlockFilterCheckPoints::new_builder()
                .start_number(start_number.pack())
                .build();
            packed::BlockFilterMessage::new_builder()
                .set(content)
                .build()
        };

        info!("Send get block filter check points request to node");
        net.send(&node, SupportProtocols::Filter, request.as_bytes());

        let (_, _, data) = net.receive_timeout(&node, Duration::from_secs(10)).unwrap();
        match packed::BlockFilterMessageReader::from_slice(&data) {
            Ok(msg) => match msg.to_enum() {
                packed::BlockFilterMessageUnionReader::BlockFilterCheckPoints(reader) => {
                    let resp_start_number: u64 = reader.start_number().unpack();
                    assert_eq!(start_number, resp_start_number);
                    let hashes: Vec<packed::Byte32> = reader
                        .block_filter_hashes()
                        .iter()
                        .map(|item| item.to_entity())
                        .collect();
                    info!("start_number matched");
                    assert_eq!(
                        hashes.len(),
                        (points_num + 1) as usize,
                        "hashes length not match"
                    );
                    for i in 0..=points_num {
                        let number = i * CHECK_POINT_INTERVAL;
                        let header = node.get_header_by_number(number);
                        let block_filter = node.get_block_filter(header.hash());
                        let expected_hash = blake2b_256(block_filter.as_ref());
                        assert_eq!(
                            &expected_hash,
                            hashes[i as usize].as_slice(),
                            "block number: {}",
                            number,
                        );
                    }
                    info!("block_filter_hashes matched");
                }
                _ => panic!("unexpected message"),
            },
            _ => panic!("unexpected message"),
        }
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.store.block_filter_enable = true;
    }
}

pub struct GetBlockFilterHashes;

impl Spec for GetBlockFilterHashes {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = nodes.pop().unwrap();
        let points_num = 2;
        node.mine(CHECK_POINT_INTERVAL * points_num + 1);

        let mut net = Net::new(
            self.name(),
            node.consensus(),
            vec![SupportProtocols::Filter],
        );
        net.connect(&node);
        let start_number: u64 = 42;
        let request = {
            let content = packed::GetBlockFilterHashes::new_builder()
                .start_number(start_number.pack())
                .build();
            packed::BlockFilterMessage::new_builder()
                .set(content)
                .build()
        };

        info!("Send get block filter check points request to node");
        net.send(&node, SupportProtocols::Filter, request.as_bytes());

        let (_, _, data) = net.receive_timeout(&node, Duration::from_secs(10)).unwrap();
        match packed::BlockFilterMessageReader::from_slice(&data) {
            Ok(msg) => match msg.to_enum() {
                packed::BlockFilterMessageUnionReader::BlockFilterHashes(reader) => {
                    let resp_start_number: u64 = reader.start_number().unpack();
                    assert_eq!(start_number, resp_start_number);
                    let hashes: Vec<packed::Byte32> = reader
                        .block_filter_hashes()
                        .iter()
                        .map(|item| item.to_entity())
                        .collect();
                    info!("start_number matched");

                    let parent_block_filter_hash = reader.parent_block_filter_hash().to_entity();
                    {
                        let header = node.get_header_by_number(start_number - 1);
                        let block_filter = node.get_block_filter(header.hash());
                        let expected_parent_hash = blake2b_256(block_filter.as_ref());
                        assert_eq!(&expected_parent_hash, parent_block_filter_hash.as_slice());
                    }
                    info!("parent_block_filter_hash matched");

                    assert_eq!(hashes.len(), BATCH_SIZE as usize, "hashes length not match");
                    for i in 0..BATCH_SIZE {
                        let number = start_number + i;
                        let header = node.get_header_by_number(number);
                        let block_filter = node.get_block_filter(header.hash());
                        let expected_hash = blake2b_256(block_filter.as_ref());
                        assert_eq!(
                            &expected_hash,
                            hashes[i as usize].as_slice(),
                            "block number: {}",
                            number,
                        );
                    }
                    info!("block_filter_hashes matched");
                }
                _ => panic!("unexpected message"),
            },
            _ => panic!("unexpected message"),
        }
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.store.block_filter_enable = true;
    }
}
