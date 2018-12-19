use libp2p::core::{AddrComponent, Multiaddr};
use std::net::IpAddr;

#[derive(Hash, Eq, PartialEq, Debug)]
pub enum Group {
    NoGroup,
    LocalNetwork,
    IP4([u8; 2]),
    IP6([u8; 4]),
}

pub trait NetworkGroup {
    fn network_group(&self) -> Group;
}

fn extract_ip_addr(addr: &Multiaddr) -> Option<IpAddr> {
    for addr_component in addr {
        match addr_component {
            AddrComponent::IP4(ipv4) => return Some(IpAddr::V4(ipv4)),
            AddrComponent::IP6(ipv6) => return Some(IpAddr::V6(ipv6)),
            _ => (),
        }
    }
    None
}

impl NetworkGroup for Multiaddr {
    fn network_group(&self) -> Group {
        if let Some(ip_addr) = extract_ip_addr(self) {
            if ip_addr.is_loopback() {
                return Group::LocalNetwork;
            }
            // TODO uncomment after ip feature stable
            // if !ip_addr.is_global() {
            //     // Global NetworkGroup
            //     return Group::GlobalNetwork
            // }

            // IPv4 NetworkGroup
            if let IpAddr::V4(ipv4) = ip_addr {
                let bits = ipv4.octets();
                return Group::IP4([bits[0], bits[1]]);
            }
            // IPv6 NetworkGroup
            if let IpAddr::V6(ipv6) = ip_addr {
                if let Some(ipv4) = ipv6.to_ipv4() {
                    let bits = ipv4.octets();
                    return Group::IP4([bits[0], bits[1]]);
                }
                let bits = ipv6.octets();
                return Group::IP6([bits[0], bits[1], bits[2], bits[3]]);
            }
        }
        // Can't group addr
        Group::NoGroup
    }
}
