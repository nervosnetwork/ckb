use p2p::multiaddr::{MultiAddr, Protocol};

#[derive(Default)]
pub struct NetworkAddresses {
    pub regular_addresses: Vec<MultiAddr>,

    // onion addresses can't be solved by multiaddr_to_socketaddr or socketaddr_to_multiaddr
    pub onion_addresses: Vec<MultiAddr>,
}

impl NetworkAddresses {
    pub fn push(&mut self, address: MultiAddr) {
        if address
            .iter()
            .any(|proto| matches!(proto, Protocol::Onion3(_)))
        {
            self.onion_addresses.push(address);
        } else {
            self.regular_addresses.push(address);
        }
    }

    // contains
    pub fn contains(&self, address: &MultiAddr) -> bool {
        self.regular_addresses.contains(address) || self.onion_addresses.contains(address)
    }
}

impl IntoIterator for NetworkAddresses {
    type Item = MultiAddr;
    type IntoIter = std::vec::IntoIter<MultiAddr>;

    fn into_iter(self) -> Self::IntoIter {
        self.regular_addresses
            .into_iter()
            .chain(self.onion_addresses.into_iter())
            .collect::<Vec<_>>()
            .into_iter()
    }
}

// convert Vec<MultiAddr> to NetworkAddresses
impl From<Vec<MultiAddr>> for NetworkAddresses {
    fn from(addresses: Vec<MultiAddr>) -> Self {
        let mut regular_addresses = Vec::new();
        let mut onion_addresses = Vec::new();
        for address in addresses {
            if address
                .iter()
                .any(|proto| matches!(proto, Protocol::Onion3(_)))
            {
                onion_addresses.push(address);
            } else {
                regular_addresses.push(address);
            }
        }
        NetworkAddresses {
            regular_addresses,
            onion_addresses,
        }
    }
}

// convert NetworkAddresses to Vec<MultiAddr>
impl From<NetworkAddresses> for Vec<MultiAddr> {
    fn from(addresses: NetworkAddresses) -> Self {
        let mut result = addresses.regular_addresses;
        result.extend(addresses.onion_addresses);
        result
    }
}
