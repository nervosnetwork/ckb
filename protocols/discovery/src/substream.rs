use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};
use std::{
    convert::TryFrom,
    pin::Pin,
    task::{Context, Poll},
};

use ckb_logger::{debug, trace, warn};
use futures::{channel::mpsc::Receiver, Sink, Stream};
use p2p::multiaddr::{Multiaddr, Protocol};
use p2p::{
    bytes::{BufMut, BytesMut},
    context::ProtocolContextMutRef,
    error::Error,
    service::{ServiceControl, SessionType},
    utils::multiaddr_to_socketaddr,
    ProtocolId, SessionId,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

use crate::addr::{AddrKnown, AddressManager, Misbehavior, RawAddr};
use crate::protocol::{DiscoveryCodec, DiscoveryMessage, Node, Nodes};

// FIXME: should be a more high level version number
const VERSION: u32 = 0;
// The maximum number of new addresses to accumulate before announcing.
const MAX_ADDR_TO_SEND: usize = 1000;
// Every 24 hours send announce nodes message
const ANNOUNCE_INTERVAL: u64 = 3600 * 24;
const ANNOUNCE_THRESHOLD: usize = 10;

// The maximum number addresses in on Nodes item
const MAX_ADDRS: usize = 3;

#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub struct SubstreamKey {
    pub(crate) direction: SessionType,
    pub(crate) session_id: SessionId,
    pub(crate) proto_id: ProtocolId,
}

pub struct StreamHandle {
    data_buf: BytesMut,
    proto_id: ProtocolId,
    session_id: SessionId,
    pub(crate) receiver: Receiver<Vec<u8>>,
    pub(crate) sender: ServiceControl,
}

impl AsyncRead for StreamHandle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        for _ in 0..10 {
            match Pin::new(&mut self.receiver).as_mut().poll_next(cx) {
                Poll::Ready(Some(data)) => {
                    self.data_buf.reserve(data.len());
                    self.data_buf.put(data.as_slice());
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                }
                Poll::Pending => {
                    break;
                }
            }
        }
        let n = std::cmp::min(buf.len(), self.data_buf.len());
        if n == 0 {
            return Poll::Pending;
        }
        let b = self.data_buf.split_to(n);
        buf[..n].copy_from_slice(&b);
        Poll::Ready(Ok(n))
    }
}

impl AsyncWrite for StreamHandle {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.sender
            .send_message_to(self.session_id, self.proto_id, BytesMut::from(buf).freeze())
            .map(|()| buf.len())
            .map_err(|e| {
                if let Error::IoError(e) = e {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        e
                    } else {
                        io::ErrorKind::BrokenPipe.into()
                    }
                } else {
                    io::ErrorKind::BrokenPipe.into()
                }
            })
            .into()
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub struct SubstreamValue {
    framed_stream: Framed<StreamHandle, DiscoveryCodec>,
    // received pending messages
    pub(crate) pending_messages: VecDeque<DiscoveryMessage>,
    pub(crate) addr_known: AddrKnown,
    // FIXME: Remote listen address, resolved by id protocol
    pub(crate) remote_addr: RemoteAddress,
    pub(crate) announce: bool,
    pub(crate) last_announce: Option<Instant>,
    pub(crate) announce_multiaddrs: Vec<Multiaddr>,
    session_id: SessionId,
    announce_interval: Duration,
    received_get_nodes: bool,
    received_nodes: bool,
    remote_closed: bool,
}

impl SubstreamValue {
    pub(crate) fn new(
        direction: SessionType,
        substream: Substream,
        max_known: usize,
        query_cycle: Option<Duration>,
    ) -> SubstreamValue {
        let session_id = substream.stream.session_id;
        let mut pending_messages = VecDeque::default();
        debug!("direction: {:?}", direction);
        let mut addr_known = AddrKnown::new(max_known);
        let remote_addr = if direction.is_outbound() {
            pending_messages.push_back(DiscoveryMessage::GetNodes {
                version: VERSION,
                count: MAX_ADDR_TO_SEND as u32,
                listen_port: substream.listen_port,
            });
            addr_known.insert(RawAddr::from(
                multiaddr_to_socketaddr(&substream.remote_addr).unwrap(),
            ));

            RemoteAddress::Listen(substream.remote_addr)
        } else {
            RemoteAddress::Init(substream.remote_addr)
        };

        SubstreamValue {
            framed_stream: Framed::new(substream.stream, DiscoveryCodec::default()),
            last_announce: None,
            announce_interval: query_cycle
                .unwrap_or_else(|| Duration::from_secs(ANNOUNCE_INTERVAL)),
            pending_messages,
            addr_known,
            remote_addr,
            session_id,
            announce: false,
            announce_multiaddrs: Vec::new(),
            received_get_nodes: false,
            received_nodes: false,
            remote_closed: false,
        }
    }

    fn remote_raw_addr(&self) -> Option<RawAddr> {
        multiaddr_to_socketaddr(self.remote_addr.to_inner()).map(RawAddr::from)
    }

    pub(crate) fn check_timer(&mut self) {
        if self
            .last_announce
            .map(|time| time.elapsed() > self.announce_interval)
            .unwrap_or(true)
        {
            debug!("announce this session: {:?}", self.session_id);
            self.announce = true;
        }
    }

    pub(crate) fn send_messages(&mut self, cx: &mut Context) -> Result<(), io::Error> {
        let mut sink = Pin::new(&mut self.framed_stream);

        while let Some(message) = self.pending_messages.pop_front() {
            debug!("Discovery sending message: {}", message);

            match sink.as_mut().poll_ready(cx)? {
                Poll::Pending => {
                    self.pending_messages.push_front(message);
                    return Ok(());
                }
                Poll::Ready(()) => {
                    sink.as_mut().start_send(message)?;
                }
            }
        }
        let _ = sink.as_mut().poll_flush(cx)?;
        Ok(())
    }

    pub(crate) fn handle_message<M: AddressManager>(
        &mut self,
        message: DiscoveryMessage,
        addr_mgr: &mut M,
    ) -> Result<Option<Nodes>, io::Error> {
        match message {
            DiscoveryMessage::GetNodes { listen_port, .. } => {
                if self.received_get_nodes {
                    // TODO: misbehavior
                    if addr_mgr
                        .misbehave(self.session_id, Misbehavior::DuplicateGetNodes)
                        .is_disconnect()
                    {
                        // TODO: more clear error type
                        warn!("Already received get nodes");
                        return Err(io::ErrorKind::Other.into());
                    }
                } else {
                    // TODO: magic number
                    // must get the item first, otherwise it is possible to load
                    // the address of peer listen.
                    let mut items = addr_mgr.get_random(2500);

                    // change client random outbound port to client listen port
                    debug!("listen port: {:?}", listen_port);
                    if let Some(port) = listen_port {
                        self.remote_addr.update_port(port);
                        if let Some(raw_addr) = self.remote_raw_addr() {
                            self.addr_known.insert(raw_addr);
                        }
                        // add client listen address to manager
                        if let RemoteAddress::Listen(ref addr) = self.remote_addr {
                            addr_mgr.add_new_addr(self.session_id, addr.clone());
                        }
                    }

                    while items.len() > 1000 {
                        if let Some(last_item) = items.pop() {
                            let idx = rand::random::<usize>() % 1000;
                            items[idx] = last_item;
                        }
                    }
                    let items = items
                        .into_iter()
                        .map(|addr| Node {
                            addresses: vec![addr],
                        })
                        .collect::<Vec<_>>();
                    let nodes = Nodes {
                        announce: false,
                        items,
                    };
                    self.pending_messages
                        .push_back(DiscoveryMessage::Nodes(nodes));
                    self.received_get_nodes = true;
                }
            }
            DiscoveryMessage::Nodes(nodes) => {
                for item in &nodes.items {
                    if item.addresses.len() > MAX_ADDRS {
                        let misbehavior = Misbehavior::TooManyAddresses(item.addresses.len());
                        if addr_mgr
                            .misbehave(self.session_id, misbehavior)
                            .is_disconnect()
                        {
                            // TODO: more clear error type
                            return Err(io::ErrorKind::Other.into());
                        }
                    }
                }

                if nodes.announce {
                    if nodes.items.len() > ANNOUNCE_THRESHOLD {
                        warn!("Nodes items more than {}", ANNOUNCE_THRESHOLD);
                        // TODO: misbehavior
                        let misbehavior = Misbehavior::TooManyItems {
                            announce: nodes.announce,
                            length: nodes.items.len(),
                        };
                        if addr_mgr
                            .misbehave(self.session_id, misbehavior)
                            .is_disconnect()
                        {
                            // TODO: more clear error type
                            return Err(io::ErrorKind::Other.into());
                        }
                    } else {
                        return Ok(Some(nodes));
                    }
                } else if self.received_nodes {
                    warn!("already received Nodes(announce=false) message");
                    // TODO: misbehavior
                    if addr_mgr
                        .misbehave(self.session_id, Misbehavior::DuplicateFirstNodes)
                        .is_disconnect()
                    {
                        // TODO: more clear error type
                        return Err(io::ErrorKind::Other.into());
                    }
                } else if nodes.items.len() > MAX_ADDR_TO_SEND {
                    warn!(
                        "Too many items (announce=false) length={}",
                        nodes.items.len()
                    );
                    // TODO: misbehavior
                    let misbehavior = Misbehavior::TooManyItems {
                        announce: nodes.announce,
                        length: nodes.items.len(),
                    };

                    if addr_mgr
                        .misbehave(self.session_id, misbehavior)
                        .is_disconnect()
                    {
                        // TODO: more clear error type
                        return Err(io::ErrorKind::Other.into());
                    }
                } else {
                    self.received_nodes = true;
                    return Ok(Some(nodes));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn receive_messages<M: AddressManager + Unpin>(
        &mut self,
        cx: &mut Context,
        addr_mgr: &mut M,
    ) -> Result<Option<(SessionId, Vec<Nodes>)>, io::Error> {
        if self.remote_closed {
            return Ok(None);
        }

        let mut nodes_list = Vec::new();
        loop {
            match Pin::new(&mut self.framed_stream).as_mut().poll_next(cx) {
                Poll::Ready(Some(res)) => {
                    let message = res?;
                    trace!("received message {}", message);
                    if let Some(nodes) = self.handle_message(message, addr_mgr)? {
                        // Add to known address list
                        for node in &nodes.items {
                            for addr in &node.addresses {
                                trace!("received address: {}", addr);
                                self.addr_known
                                    .insert(RawAddr::try_from(addr.clone()).unwrap());
                            }
                        }
                        nodes_list.push(nodes);
                    }
                }
                Poll::Ready(None) => {
                    debug!("remote closed");
                    self.remote_closed = true;
                    break;
                }
                Poll::Pending => {
                    break;
                }
            }
        }
        Ok(Some((self.session_id, nodes_list)))
    }
}

pub struct Substream {
    pub remote_addr: Multiaddr,
    pub direction: SessionType,
    pub stream: StreamHandle,
    pub listen_port: Option<u16>,
}

impl Substream {
    pub fn new(context: ProtocolContextMutRef, receiver: Receiver<Vec<u8>>) -> Substream {
        let stream = StreamHandle {
            data_buf: BytesMut::default(),
            proto_id: context.proto_id,
            session_id: context.session.id,
            receiver,
            sender: context.control().clone(),
        };
        let listen_port = if context.session.ty.is_outbound() {
            context
                .listens()
                .iter()
                .map(|address| multiaddr_to_socketaddr(address).unwrap().port())
                .nth(0)
        } else {
            None
        };
        Substream {
            remote_addr: context.session.address.clone(),
            direction: context.session.ty,
            stream,
            listen_port,
        }
    }

    pub fn key(&self) -> SubstreamKey {
        SubstreamKey {
            direction: self.direction,
            session_id: self.stream.session_id,
            proto_id: self.stream.proto_id,
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub(crate) enum RemoteAddress {
    /// Inbound init remote address
    Init(Multiaddr),
    /// Outbound init remote address or Inbound listen address
    Listen(Multiaddr),
}

impl RemoteAddress {
    fn to_inner(&self) -> &Multiaddr {
        match self {
            RemoteAddress::Init(ref addr) | RemoteAddress::Listen(ref addr) => addr,
        }
    }

    pub(crate) fn into_inner(self) -> Multiaddr {
        match self {
            RemoteAddress::Init(addr) | RemoteAddress::Listen(addr) => addr,
        }
    }

    fn update_port(&mut self, port: u16) {
        if let RemoteAddress::Init(ref addr) = self {
            let addr = addr
                .into_iter()
                .map(|proto| {
                    match proto {
                        // TODO: other transport, UDP for example
                        Protocol::TCP(_) => Protocol::TCP(port),
                        value => value,
                    }
                })
                .collect();
            *self = RemoteAddress::Listen(addr);
        }
    }
}
