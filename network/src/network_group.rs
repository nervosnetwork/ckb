use libp2p::core::{AddrComponent, Multiaddr};
use std::net::IpAddr;

pub type Group = Vec<u8>;

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
                // Local NetworkGroup
                return vec![1];
            }
            // TODO uncomment after ip feature stable
            // if !ip_addr.is_global() {
            //     // Global NetworkGroup
            //     return vec![2]
            // }

            // IPv4 NetworkGroup
            if let IpAddr::V4(ipv4) = ip_addr {
                return ipv4.octets()[0..2].to_vec();
            }
            // IPv6 NetworkGroup
            if let IpAddr::V6(ipv6) = ip_addr {
                if let Some(ipv4) = ipv6.to_ipv4() {
                    return ipv4.octets()[0..2].to_vec();
                }
                return ipv6.octets()[0..4].to_vec();
            }
        }
        // Can't group addr
        vec![0]
    }
}
