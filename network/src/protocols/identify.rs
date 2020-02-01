// use crate::peer_store::Behaviour;
use crate::{network::FEELER_PROTOCOL_ID, NetworkState, PeerIdentifyInfo};
use ckb_logger::{debug, trace};
use ckb_types::{bytes::Bytes, packed, prelude::*};
use p2p::{
    context::ProtocolContextMutRef,
    multiaddr::{Multiaddr, Protocol},
    secio::{PeerId, PublicKey},
    service::{SessionType, TargetProtocol},
    utils::{is_reachable, multiaddr_to_socketaddr},
};
use p2p_identify::{Callback, MisbehaveResult, Misbehavior};
use std::{sync::Arc, time::Duration};

const MAX_RETURN_LISTEN_ADDRS: usize = 10;
const BAN_ON_NOT_SAME_NET: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub(crate) struct IdentifyCallback {
    network_state: Arc<NetworkState>,
    identify: Identify,
}

impl IdentifyCallback {
    pub(crate) fn new(
        network_state: Arc<NetworkState>,
        name: String,
        client_version: String,
    ) -> IdentifyCallback {
        let flags = Flags(Flag::FullNode as u64);

        IdentifyCallback {
            network_state,
            identify: Identify::new(name, flags, client_version),
        }
    }

    fn listen_addrs(&self) -> Vec<Multiaddr> {
        let mut addrs = self.network_state.public_addrs(MAX_RETURN_LISTEN_ADDRS * 2);
        addrs.sort_by(|a, b| a.1.cmp(&b.1));
        addrs
            .into_iter()
            .take(MAX_RETURN_LISTEN_ADDRS)
            .map(|(addr, _)| addr)
            .collect::<Vec<_>>()
    }
}

impl Callback for IdentifyCallback {
    fn identify(&mut self) -> &[u8] {
        self.identify.encode()
    }

    fn received_identify(
        &mut self,
        context: &mut ProtocolContextMutRef,
        identify: &[u8],
    ) -> MisbehaveResult {
        match self.identify.verify(identify) {
            None => {
                self.network_state.ban_session(
                    context.control(),
                    context.session.id,
                    BAN_ON_NOT_SAME_NET,
                    "The nodes are not on the same network".to_string(),
                );
                MisbehaveResult::Disconnect
            }
            Some((flags, client_version)) => {
                let registry_client_version = |version: String| {
                    self.network_state.with_peer_registry_mut(|registry| {
                        if let Some(peer) = registry.get_peer_mut(context.session.id) {
                            peer.identify_info = Some(PeerIdentifyInfo {
                                client_version: version,
                            })
                        }
                    });
                };

                if context.session.ty.is_outbound() {
                    let peer_id = context
                        .session
                        .remote_pubkey
                        .as_ref()
                        .map(PublicKey::peer_id)
                        .expect("Secio must enabled");
                    if self
                        .network_state
                        .with_peer_registry(|reg| reg.is_feeler(&peer_id))
                    {
                        let _ = context.open_protocols(
                            context.session.id,
                            TargetProtocol::Single(FEELER_PROTOCOL_ID.into()),
                        );
                    } else if flags.contains(self.identify.flags) {
                        registry_client_version(client_version);

                        // The remote end can support all local protocols.
                        let protos = self
                            .network_state
                            .get_protocol_ids(|id| id != FEELER_PROTOCOL_ID.into());

                        let _ = context
                            .open_protocols(context.session.id, TargetProtocol::Multi(protos));
                    } else {
                        // The remote end cannot support all local protocols.
                        return MisbehaveResult::Disconnect;
                    }
                } else {
                    registry_client_version(client_version);
                }
                MisbehaveResult::Continue
            }
        }
    }

    /// Get local listen addresses
    fn local_listen_addrs(&mut self) -> Vec<Multiaddr> {
        self.listen_addrs()
    }

    fn add_remote_listen_addrs(&mut self, peer_id: &PeerId, addrs: Vec<Multiaddr>) {
        trace!(
            "got remote listen addrs from peer_id={:?}, addrs={:?}",
            peer_id,
            addrs,
        );
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg
                .get_key_by_peer_id(peer_id)
                .and_then(|session_id| reg.get_peer_mut(session_id))
            {
                peer.listened_addrs = addrs.clone();
            }
        });
        self.network_state.with_peer_store_mut(|peer_store| {
            for addr in addrs {
                if let Err(err) = peer_store.add_addr(peer_id.clone(), addr) {
                    debug!("Failed to add addrs to peer_store {:?} {:?}", err, peer_id);
                }
            }
        })
    }

    fn add_observed_addr(
        &mut self,
        peer_id: &PeerId,
        addr: Multiaddr,
        ty: SessionType,
    ) -> MisbehaveResult {
        debug!(
            "peer({:?}, {:?}) reported observed addr {}",
            peer_id, ty, addr,
        );

        if ty.is_inbound() {
            // The address already been discovered by other peer
            return MisbehaveResult::Continue;
        }

        // observed addr is not a reachable ip
        if !multiaddr_to_socketaddr(&addr)
            .map(|socket_addr| is_reachable(socket_addr.ip()))
            .unwrap_or(false)
        {
            return MisbehaveResult::Continue;
        }

        let observed_addrs_iter = self
            .listen_addrs()
            .into_iter()
            .filter_map(|listen_addr| multiaddr_to_socketaddr(&listen_addr))
            .map(|socket_addr| {
                addr.iter()
                    .filter_map(|proto| match proto {
                        Protocol::P2P(_) => None,
                        Protocol::TCP(_) => Some(Protocol::TCP(socket_addr.port())),
                        value => Some(value),
                    })
                    .collect::<Multiaddr>()
            });
        self.network_state.add_observed_addrs(observed_addrs_iter);
        // NOTE: for future usage
        MisbehaveResult::Continue
    }

    fn misbehave(&mut self, _peer_id: &PeerId, _kind: Misbehavior) -> MisbehaveResult {
        MisbehaveResult::Disconnect
    }
}

#[derive(Clone)]
struct Identify {
    name: String,
    client_version: String,
    flags: Flags,
    encode_data: Bytes,
}

impl Identify {
    fn new(name: String, flags: Flags, client_version: String) -> Self {
        Identify {
            name,
            client_version,
            flags,
            encode_data: Bytes::default(),
        }
    }

    fn encode(&mut self) -> &[u8] {
        if self.encode_data.is_empty() {
            self.encode_data = packed::Identify::new_builder()
                .name(self.name.as_str().pack())
                .flag(self.flags.0.pack())
                .client_version(self.client_version.as_str().pack())
                .build()
                .as_bytes();
        }

        &self.encode_data
    }

    fn verify<'a>(&self, data: &'a [u8]) -> Option<(Flags, String)> {
        let reader = packed::IdentifyReader::from_slice(data).ok()?;

        let name = reader.name().as_utf8().ok()?.to_owned();
        if self.name != name {
            debug!("Not the same chain, self: {}, remote: {}", self.name, name);
            return None;
        }

        let flag: u64 = reader.flag().unpack();
        if flag == 0 {
            return None;
        }

        let raw_client_version = reader.client_version().as_utf8().ok()?.to_owned();

        Some((Flags::from(flag), raw_client_version))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u64)]
enum Flag {
    /// Support all protocol
    FullNode = 0x1,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
struct Flags(u64);

impl Flags {
    /// Check if contains a target flag
    fn contains(self, flags: Flags) -> bool {
        (self.0 & flags.0) == flags.0
    }
}

impl From<Flag> for Flags {
    fn from(value: Flag) -> Flags {
        Flags(value as u64)
    }
}

impl From<u64> for Flags {
    fn from(value: u64) -> Flags {
        Flags(value)
    }
}
