// use crate::peer_store::Behaviour;
use crate::{network::FEELER_PROTOCOL_ID, NetworkState};
use ckb_logger::{debug, trace};
use ckb_protocol::{
    flatbuffers::FlatBufferBuilder, get_root, Identify as FbsIdentify,
    IdentifyBuilder as FbsIdentifyBuilder,
};
use p2p::{
    context::ProtocolContextMutRef,
    multiaddr::{Multiaddr, Protocol},
    secio::PeerId,
    service::{SessionType, TargetProtocol},
    utils::{is_reachable, multiaddr_to_socketaddr},
};
use p2p_identify::{Callback, MisbehaveResult, Misbehavior};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_RETURN_LISTEN_ADDRS: usize = 10;

#[derive(Clone)]
pub(crate) struct IdentifyCallback {
    network_state: Arc<NetworkState>,
    identify: Identify,
    // local listen addresses for scoring and for rpc output
    remote_listen_addrs: HashMap<PeerId, Vec<Multiaddr>>,
}

impl IdentifyCallback {
    pub(crate) fn new(network_state: Arc<NetworkState>, name: String) -> IdentifyCallback {
        let flags = Flags(Flag::FullNode as u64);

        IdentifyCallback {
            network_state,
            identify: Identify::new(name, flags),
            remote_listen_addrs: HashMap::default(),
        }
    }

    fn listen_addrs(&self) -> Vec<Multiaddr> {
        let mut addrs = self
            .network_state
            .listened_addresses(MAX_RETURN_LISTEN_ADDRS * 2);
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
            None => MisbehaveResult::Disconnect,
            Some(flags) => {
                if context.session.ty.is_outbound() {
                    if flags.contains(self.identify.flags) {
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
        self.remote_listen_addrs
            .insert(peer_id.clone(), addrs.clone());
        self.network_state.with_peer_store_mut(|peer_store| {
            for addr in addrs {
                peer_store.add_discovered_addr(&peer_id, addr);
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

        for transformed_addr in self
            .listen_addrs()
            .into_iter()
            .filter_map(|listen_addr| multiaddr_to_socketaddr(&listen_addr))
            .filter(|socket_addr| is_reachable(socket_addr.ip()))
            .map(|socket_addr| socket_addr.port())
            .map(|listen_port| {
                addr.iter()
                    .filter_map(|proto| match proto {
                        // Replace only it's an outbound connnection
                        Protocol::P2p(_) => None,
                        Protocol::Tcp(_) => Some(Protocol::Tcp(listen_port)),
                        value => Some(value),
                    })
                    .collect::<Multiaddr>()
            })
        {
            debug!("identify add transformed addr: {:?}", transformed_addr);
            let local_peer_id = self.network_state.local_peer_id();
            self.network_state.with_peer_store_mut(|peer_store| {
                peer_store.add_discovered_addr(local_peer_id, transformed_addr);
            });
        }
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
    flags: Flags,
    encode_data: Vec<u8>,
}

impl Identify {
    fn new(name: String, flags: Flags) -> Self {
        Identify {
            name,
            flags,
            encode_data: Vec::default(),
        }
    }

    fn encode(&mut self) -> &[u8] {
        if self.encode_data.is_empty() {
            let mut fbb = FlatBufferBuilder::new();
            let name = fbb.create_string(&self.name);

            let mut builder = FbsIdentifyBuilder::new(&mut fbb);

            builder.add_flag(self.flags.0);
            builder.add_name(name);

            let data = builder.finish();
            fbb.finish(data, None);
            self.encode_data = fbb.finished_data().to_vec();
        }

        &self.encode_data
    }

    fn verify(&self, data: &[u8]) -> Option<Flags> {
        let fbs_message = get_root::<FbsIdentify>(data).ok()?;

        match (fbs_message.name(), fbs_message.flag()) {
            (Some(raw_name), flag) => {
                if self.name != raw_name {
                    debug!(
                        "Not the same chain, self: {}, remote: {}",
                        self.name, raw_name
                    );
                    return None;
                }

                if flag == 0 {
                    return None;
                }

                Some(Flags::from(flag))
            }
            _ => None,
        }
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
    /// Add a flag
    #[allow(dead_code)]
    pub fn add(&mut self, flag: Flag) {
        self.0 |= flag as u64;
    }

    /// Remove a flag
    #[allow(dead_code)]
    pub fn remove(&mut self, flag: Flag) {
        self.0 ^= flag as u64;
    }

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
