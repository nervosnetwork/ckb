use std::convert::TryFrom;

use p2p::{
    bytes::{Bytes, BytesMut},
    multiaddr::Multiaddr,
};
use tokio_util::codec::length_delimited::LengthDelimitedCodec;
use tokio_util::codec::{Decoder, Encoder};

use ckb_logger::debug;
use ckb_types::{packed, prelude::*};

pub(crate) fn encode(data: DiscoveryMessage) -> Bytes {
    // Length Delimited Codec is not a mandatory requirement.
    // For historical reasons, this must exist as compatibility
    let mut codec = LengthDelimitedCodec::new();
    let mut bytes = BytesMut::new();
    codec
        .encode(data.encode(), &mut bytes)
        .expect("encode must be success");
    bytes.freeze()
}

pub(crate) fn decode(data: &mut BytesMut) -> Option<DiscoveryMessage> {
    // Length Delimited Codec is not a mandatory requirement.
    // For historical reasons, this must exist as compatibility
    let mut codec = LengthDelimitedCodec::new();
    match codec.decode(data) {
        Ok(Some(frame)) => DiscoveryMessage::decode(&frame),
        Ok(None) => None,
        Err(err) => {
            debug!("decode error: {:?}", err);
            None
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiscoveryMessage {
    GetNodes {
        version: u32,
        count: u32,
        listen_port: Option<u16>,
    },
    Nodes(Nodes),
}

impl DiscoveryMessage {
    pub fn encode(self) -> Bytes {
        let playload = match self {
            DiscoveryMessage::GetNodes {
                version,
                count,
                listen_port,
            } => {
                let version_le = version.to_le_bytes();
                let count_le = count.to_le_bytes();
                let version = packed::Uint32::new_builder()
                    .nth0(version_le[0].into())
                    .nth1(version_le[1].into())
                    .nth2(version_le[2].into())
                    .nth3(version_le[3].into())
                    .build();
                let count = packed::Uint32::new_builder()
                    .nth0(count_le[0].into())
                    .nth1(count_le[1].into())
                    .nth2(count_le[2].into())
                    .nth3(count_le[3].into())
                    .build();
                let listen_port = packed::PortOpt::new_builder()
                    .set(listen_port.map(|port| {
                        let port_le = port.to_le_bytes();
                        packed::Uint16::new_builder()
                            .nth0(port_le[0].into())
                            .nth1(port_le[1].into())
                            .build()
                    }))
                    .build();
                let get_node = packed::GetNodes::new_builder()
                    .listen_port(listen_port)
                    .count(count)
                    .version(version)
                    .build();
                packed::DiscoveryPayload::new_builder()
                    .set(get_node)
                    .build()
            }
            DiscoveryMessage::Nodes(Nodes { announce, items }) => {
                let bool_ = if announce { 1u8 } else { 0 };
                let announce = packed::DiscoveryBool::new_builder()
                    .set([bool_.into()])
                    .build();
                let mut item_vec = Vec::with_capacity(items.len());
                for item in items {
                    let mut vec_addrs = Vec::with_capacity(item.addresses.len());
                    for addr in item.addresses {
                        vec_addrs.push(
                            packed::Bytes::new_builder()
                                .set(addr.to_vec().into_iter().map(Into::into).collect())
                                .build(),
                        )
                    }
                    let bytes_vec = packed::BytesVec::new_builder().set(vec_addrs).build();
                    let node = packed::Node::new_builder().addresses(bytes_vec).build();
                    item_vec.push(node)
                }
                let items = packed::NodeVec::new_builder().set(item_vec).build();
                let nodes = packed::Nodes::new_builder()
                    .announce(announce)
                    .items(items)
                    .build();
                packed::DiscoveryPayload::new_builder().set(nodes).build()
            }
        };

        packed::DiscoveryMessage::new_builder()
            .payload(playload)
            .build()
            .as_bytes()
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        let reader = packed::DiscoveryMessageReader::from_compatible_slice(data).ok()?;
        match reader.payload().to_enum() {
            packed::DiscoveryPayloadUnionReader::GetNodes(reader) => {
                let version = {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(reader.version().raw_data());
                    u32::from_le_bytes(b)
                };
                let count = {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(reader.count().raw_data());
                    u32::from_le_bytes(b)
                };
                let listen_port = reader.listen_port().to_opt().map(|port_reader| {
                    let mut b = [0u8; 2];
                    b.copy_from_slice(port_reader.raw_data());
                    u16::from_le_bytes(b)
                });
                Some(DiscoveryMessage::GetNodes {
                    version,
                    count,
                    listen_port,
                })
            }
            packed::DiscoveryPayloadUnionReader::Nodes(reader) => {
                let announce = match reader.announce().as_slice()[0] {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let mut items = Vec::with_capacity(reader.items().len());
                for node_reader in reader.items().iter() {
                    let mut addresses = Vec::with_capacity(node_reader.addresses().len());
                    for address_reader in node_reader.addresses().iter() {
                        addresses
                            .push(Multiaddr::try_from(address_reader.raw_data().to_vec()).ok()?)
                    }
                    items.push(Node { addresses })
                }
                Some(DiscoveryMessage::Nodes(Nodes { announce, items }))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Nodes {
    pub(crate) announce: bool,
    pub(crate) items: Vec<Node>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    pub(crate) addresses: Vec<Multiaddr>,
}

impl std::fmt::Display for DiscoveryMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match self {
            DiscoveryMessage::GetNodes { version, count, .. } => {
                write!(
                    f,
                    "DiscoveryMessage::GetNodes(version:{}, count:{})",
                    version, count
                )?;
            }
            DiscoveryMessage::Nodes(Nodes { announce, items }) => {
                write!(
                    f,
                    "DiscoveryMessage::Nodes(announce:{}, items.length:{})",
                    announce,
                    items.len()
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::{decode, encode, DiscoveryMessage};
    use ckb_types::bytes::BytesMut;

    #[test]
    fn test_codec() {
        let msg1 = DiscoveryMessage::GetNodes {
            version: 0,
            count: 1,
            listen_port: Some(1),
        };

        let msg2 = DiscoveryMessage::GetNodes {
            version: 0,
            count: 1,
            listen_port: Some(2),
        };

        let b1 = encode(msg1.clone());

        let decode1 = decode(&mut BytesMut::from(b1.as_ref())).unwrap();
        assert_eq!(decode1, msg1);

        let b2 = encode(msg2.clone());

        let decode2 = decode(&mut BytesMut::from(b2.as_ref())).unwrap();
        assert_eq!(decode2, msg2);
    }
}
