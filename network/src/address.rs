use p2p::multiaddr::{MultiAddr, Protocol};

#[derive(Default, Clone, Debug)]
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

    // len
    pub fn len(&self) -> usize {
        self.regular_addresses.len() + self.onion_addresses.len()
    }

    // is_empty
    pub fn is_empty(&self) -> bool {
        self.regular_addresses.is_empty() && self.onion_addresses.is_empty()
    }
}

// implement iter() for NetworkAddresses, don't take ownership
impl<'a> IntoIterator for &'a NetworkAddresses {
    type Item = &'a MultiAddr;
    type IntoIter =
        std::iter::Chain<std::slice::Iter<'a, MultiAddr>, std::slice::Iter<'a, MultiAddr>>;

    fn into_iter(self) -> Self::IntoIter {
        self.regular_addresses
            .iter()
            .chain(self.onion_addresses.iter())
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
