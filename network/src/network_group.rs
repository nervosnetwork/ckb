use p2p::multiaddr::{Multiaddr, Protocol};
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

pub trait MultiaddrExt {
    fn extract_ip_addr(&self) -> Option<IpAddr>;
    fn extract_ip_addr_binary(&self) -> Option<Vec<u8>> {
        self.extract_ip_addr().map(|ip| match ip {
            IpAddr::V4(ipv4) => ipv4.octets().to_vec(),
            IpAddr::V6(ipv6) => ipv6.octets().to_vec(),
        })
    }
}

impl MultiaddrExt for Multiaddr {
    fn extract_ip_addr(&self) -> Option<IpAddr> {
        for addr_component in self {
            match addr_component {
                Protocol::Ip4(ipv4) => return Some(IpAddr::V4(ipv4)),
                Protocol::Ip6(ipv6) => return Some(IpAddr::V6(ipv6)),
                _ => (),
            }
        }
        None
    }
}

impl NetworkGroup for Multiaddr {
    fn network_group(&self) -> Group {
        if let Some(ip_addr) = self.extract_ip_addr() {
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
