use p2p::{bytes::Bytes, multiaddr::Multiaddr};

use ckb_types::{packed, prelude::*};
use std::convert::TryFrom;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentifyMessage<'a> {
    pub(crate) listen_addrs: Vec<Multiaddr>,
    pub(crate) observed_addr: Multiaddr,
    pub(crate) identify: &'a [u8],
}

impl<'a> IdentifyMessage<'a> {
    pub(crate) fn new(
        listen_addrs: Vec<Multiaddr>,
        observed_addr: Multiaddr,
        identify: &'a [u8],
    ) -> Self {
        IdentifyMessage {
            listen_addrs,
            observed_addr,
            identify,
        }
    }

    pub(crate) fn encode(self) -> Bytes {
        let identify = packed::Bytes::new_builder()
            .set(self.identify.to_vec().into_iter().map(Into::into).collect())
            .build();
        let observed_addr = packed::Address::new_builder()
            .bytes(
                packed::Bytes::new_builder()
                    .set(
                        self.observed_addr
                            .to_vec()
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                    )
                    .build(),
            )
            .build();
        let mut listen_addrs = Vec::with_capacity(self.listen_addrs.len());
        for addr in self.listen_addrs {
            listen_addrs.push(
                packed::Address::new_builder()
                    .bytes(
                        packed::Bytes::new_builder()
                            .set(addr.to_vec().into_iter().map(Into::into).collect())
                            .build(),
                    )
                    .build(),
            )
        }
        let listen_addrs = packed::AddressVec::new_builder().set(listen_addrs).build();

        packed::IdentifyMessage::new_builder()
            .listen_addrs(listen_addrs)
            .observed_addr(observed_addr)
            .identify(identify)
            .build()
            .as_bytes()
    }

    pub(crate) fn decode(data: &'a [u8]) -> Option<Self> {
        let reader = packed::IdentifyMessageReader::from_compatible_slice(data).ok()?;

        let identify = reader.identify().raw_data();
        let observed_addr =
            Multiaddr::try_from(reader.observed_addr().bytes().raw_data().to_vec()).ok()?;
        let mut listen_addrs = Vec::with_capacity(reader.listen_addrs().len());
        for addr in reader.listen_addrs().iter() {
            listen_addrs.push(Multiaddr::try_from(addr.bytes().raw_data().to_vec()).ok()?)
        }

        Some(IdentifyMessage {
            identify,
            observed_addr,
            listen_addrs,
        })
    }
}
