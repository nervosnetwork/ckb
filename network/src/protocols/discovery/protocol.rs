use p2p::{bytes::Bytes, multiaddr::Multiaddr};

use ckb_types::{packed, prelude::*};

use crate::Flags;

pub(crate) fn encode(data: DiscoveryMessage) -> Bytes {
    data.encode()
}

pub(crate) fn decode(data: &Bytes) -> Option<DiscoveryMessage> {
    DiscoveryMessage::decode(data)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiscoveryMessage {
    GetNodes {
        version: u32,
        count: u32,
        listen_port: Option<u16>,
        required_flags: Flags,
    },
    Nodes(Nodes),
}

impl DiscoveryMessage {
    pub fn encode(self) -> Bytes {
        let payload = match self {
            DiscoveryMessage::GetNodes {
                version,
                count,
                listen_port,
                required_flags,
            } => {
                let version = version.pack();
                let count = count.pack();
                let listen_port = packed::PortOpt::new_builder()
                    .set(listen_port.map(|port| {
                        let port_le = port.to_le_bytes();
                        packed::Uint16::new_builder()
                            .nth0(port_le[0].into())
                            .nth1(port_le[1].into())
                            .build()
                    }))
                    .build();
                let required_flags = required_flags.bits().pack();
                let get_node = packed::GetNodes2::new_builder()
                    .listen_port(listen_port)
                    .count(count)
                    .version(version)
                    .required_flags(required_flags)
                    .build();

                let get_node = packed::GetNodes::new_unchecked(get_node.as_bytes());
                packed::DiscoveryPayload::new_builder()
                    .set(get_node)
                    .build()
            }
            DiscoveryMessage::Nodes(Nodes { announce, items }) => {
                let bool_ = u8::from(announce);
                let announce = packed::Bool::new_builder().set([bool_.into()]).build();
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
                    let flags = item.flags.bits().pack();
                    let node = packed::Node2::new_builder()
                        .addresses(bytes_vec)
                        .flags(flags)
                        .build();
                    item_vec.push(node)
                }
                let items = packed::Node2Vec::new_builder().set(item_vec).build();
                let nodes = packed::Nodes2::new_builder()
                    .announce(announce)
                    .items(items)
                    .build();

                let nodes = packed::Nodes::new_unchecked(nodes.as_bytes());
                packed::DiscoveryPayload::new_builder().set(nodes).build()
            }
        };

        packed::DiscoveryMessage::new_builder()
            .payload(payload)
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

                let required_flags = if reader.has_extra_fields() {
                    let get_nodes2 =
                        packed::GetNodes2::from_compatible_slice(reader.as_slice()).ok()?;
                    let reader = get_nodes2.as_reader();
                    Flags::from_bits_truncate(reader.required_flags().unpack())
                } else {
                    Flags::COMPATIBILITY
                };
                Some(DiscoveryMessage::GetNodes {
                    version,
                    count,
                    listen_port,
                    required_flags,
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
                    let flags = if node_reader.has_extra_fields() {
                        let node2 =
                            packed::Node2::from_compatible_slice(node_reader.as_slice()).ok()?;
                        let reader = node2.as_reader();
                        Flags::from_bits_truncate(reader.flags().unpack())
                    } else {
                        Flags::COMPATIBILITY
                    };
                    items.push(Node { addresses, flags })
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
    pub(crate) flags: Flags,
}

impl std::fmt::Display for DiscoveryMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match self {
            DiscoveryMessage::GetNodes { version, count, .. } => {
                write!(
                    f,
                    "DiscoveryMessage::GetNodes(version:{version}, count:{count})"
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
