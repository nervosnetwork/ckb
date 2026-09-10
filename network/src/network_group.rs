use crate::{multiaddr::Multiaddr, multiaddr_to_ip_socketaddr};
use std::net::IpAddr;

#[derive(Hash, Eq, PartialEq, Debug)]
pub enum Group {
    None,
    LocalNetwork,
    IP4([u8; 2]),
    IP6([u8; 4]),
}

impl From<&Multiaddr> for Group {
    fn from(multiaddr: &Multiaddr) -> Group {
        if let Some(socket_addr) = multiaddr_to_ip_socketaddr(multiaddr) {
            let ip_addr = socket_addr.ip();
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
        Group::None
    }
}

#[cfg(test)]
mod tests {
    use super::{Group, Multiaddr};

    #[test]
    fn quic_addr_groups_by_ip_like_tcp() {
        let tcp: Multiaddr = "/ip4/192.168.0.1/tcp/42".parse().unwrap();
        let quic: Multiaddr = "/ip4/192.168.0.1/udp/42/quic-v1".parse().unwrap();

        // QUIC peers must participate in IP-based eviction grouping instead of
        // all falling into `Group::None`.
        assert!(!matches!(Group::from(&quic), Group::None));
        assert_eq!(Group::from(&tcp), Group::from(&quic));

        let quic_loopback: Multiaddr = "/ip4/127.0.0.1/udp/42/quic-v1".parse().unwrap();
        assert_eq!(Group::from(&quic_loopback), Group::LocalNetwork);
    }
}
