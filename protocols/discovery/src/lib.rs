use std::collections::{HashMap, HashSet, VecDeque};
use std::{
    convert::TryFrom,
    pin::Pin,
    task::{Context, Poll},
};

use ckb_logger::{debug, warn};
use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    prelude::*,
    stream::FusedStream,
    Stream,
};
use p2p::{
    bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::Multiaddr,
    traits::ServiceProtocol,
    utils::{is_reachable, multiaddr_to_socketaddr},
    SessionId,
};
use rand::seq::SliceRandom;
use tokio::time::Interval;

use std::time::{Duration, Instant};

const CHECK_INTERVAL: Duration = Duration::from_secs(3);

mod addr;
mod protocol;
mod substream;

pub use crate::{
    addr::{AddrKnown, AddressManager, MisbehaveResult, Misbehavior, RawAddr},
    protocol::{DiscoveryMessage, Node, Nodes},
    substream::{Substream, SubstreamKey, SubstreamValue},
};

use crate::{addr::DEFAULT_MAX_KNOWN, substream::RemoteAddress};

pub struct DiscoveryProtocol<M> {
    discovery: Option<Discovery<M>>,
    discovery_handle: DiscoveryHandle,
    discovery_senders: HashMap<SessionId, Sender<Vec<u8>>>,
}

impl<M: AddressManager + Unpin> DiscoveryProtocol<M> {
    pub fn new(discovery: Discovery<M>) -> DiscoveryProtocol<M> {
        let discovery_handle = discovery.handle();
        DiscoveryProtocol {
            discovery: Some(discovery),
            discovery_handle,
            discovery_senders: HashMap::default(),
        }
    }
}

impl<M: AddressManager + Unpin + Send + 'static> ServiceProtocol for DiscoveryProtocol<M> {
    fn init(&mut self, context: &mut ProtocolContext) {
        debug!("protocol [discovery({})]: init", context.proto_id);

        let discovery_task = self
            .discovery
            .take()
            .map(|mut discovery| {
                debug!("Start discovery future_task");
                async move {
                    loop {
                        if discovery.next().await.is_none() {
                            warn!("discovery stream shutdown");
                            break;
                        }
                    }
                }
                .boxed()
            })
            .unwrap();
        if context.future_task(discovery_task).is_err() {
            warn!("start discovery fail");
        };
    }

    fn connected(&mut self, context: ProtocolContextMutRef, _: &str) {
        let session = context.session;
        debug!(
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );

        let (sender, receiver) = channel(8);
        self.discovery_senders.insert(session.id, sender);
        let substream = Substream::new(context, receiver);
        match self.discovery_handle.substream_sender.try_send(substream) {
            Ok(_) => {
                debug!("Send substream success");
            }
            Err(err) => {
                // TODO: handle channel is full (wait for poll API?)
                warn!("Send substream failed : {:?}", err);
            }
        }
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        self.discovery_senders.remove(&context.session.id);
        debug!(
            "protocol [discovery] close on session [{}]",
            context.session.id
        );
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        debug!("[received message]: length={}", data.len());

        if let Some(ref mut sender) = self.discovery_senders.get_mut(&context.session.id) {
            // TODO: handle channel is full (wait for poll API?)
            if let Err(err) = sender.try_send(data.to_vec()) {
                if err.is_full() {
                    warn!("channel is full");
                } else if err.is_disconnected() {
                    warn!("channel is disconnected");
                } else {
                    warn!("other channel error: {:?}", err);
                }
            }
        }
    }
}

pub struct Discovery<M> {
    // Default: 5000
    max_known: usize,

    // Address Manager
    addr_mgr: M,

    // The Nodes not yet been yield
    pending_nodes: VecDeque<(SubstreamKey, SessionId, Nodes)>,

    // For manage those substreams
    substreams: HashMap<SubstreamKey, SubstreamValue>,

    // For add new substream to Discovery
    substream_sender: Sender<Substream>,
    // For add new substream to Discovery
    substream_receiver: Receiver<Substream>,

    dead_keys: HashSet<SubstreamKey>,

    dynamic_query_cycle: Option<Duration>,

    check_interval: Option<Interval>,

    global_ip_only: bool,
}

#[derive(Clone)]
pub struct DiscoveryHandle {
    pub substream_sender: Sender<Substream>,
}

impl<M: AddressManager + Unpin> Discovery<M> {
    /// Query cycle means checking and synchronizing the cycle time of the currently connected node, default is 24 hours
    pub fn new(addr_mgr: M, query_cycle: Option<Duration>) -> Discovery<M> {
        let (substream_sender, substream_receiver) = channel(8);
        Discovery {
            check_interval: None,
            max_known: DEFAULT_MAX_KNOWN,
            addr_mgr,
            pending_nodes: VecDeque::default(),
            substreams: HashMap::default(),
            substream_sender,
            substream_receiver,
            dead_keys: HashSet::default(),
            dynamic_query_cycle: query_cycle,
            global_ip_only: true,
        }
    }

    /// Turning off global ip only mode will allow any ip to be broadcast, default is true
    pub fn global_ip_only(mut self, global_ip_only: bool) -> Self {
        self.global_ip_only = global_ip_only;
        self
    }

    pub fn addr_mgr(&self) -> &M {
        &self.addr_mgr
    }

    pub fn handle(&self) -> DiscoveryHandle {
        DiscoveryHandle {
            substream_sender: self.substream_sender.clone(),
        }
    }

    fn recv_substreams(&mut self, cx: &mut Context) {
        loop {
            if self.substream_receiver.is_terminated() {
                break;
            }

            match Pin::new(&mut self.substream_receiver)
                .as_mut()
                .poll_next(cx)
            {
                Poll::Ready(Some(substream)) => {
                    let key = substream.key();
                    debug!("Received a substream: key={:?}", key);
                    let value = SubstreamValue::new(
                        key.direction,
                        substream,
                        self.max_known,
                        self.dynamic_query_cycle,
                    );
                    self.substreams.insert(key, value);
                }
                Poll::Ready(None) => unreachable!(),
                Poll::Pending => {
                    debug!("Discovery.substream_receiver Async::NotReady");
                    break;
                }
            }
        }
    }

    fn check_interval(&mut self, cx: &mut Context) {
        if self.check_interval.is_none() {
            self.check_interval = Some(tokio::time::interval(CHECK_INTERVAL));
        }
        let mut interval = self.check_interval.take().unwrap();
        loop {
            match Pin::new(&mut interval).as_mut().poll_next(cx) {
                Poll::Ready(Some(_)) => {}
                Poll::Ready(None) => {
                    debug!("Discovery check_interval poll finished");
                    break;
                }
                Poll::Pending => break,
            }
        }
        self.check_interval = Some(interval);
    }

    fn poll_substreams(&mut self, cx: &mut Context, announce_multiaddrs: &mut Vec<Multiaddr>) {
        let announce_fn =
            |announce_multiaddrs: &mut Vec<Multiaddr>, global_ip_only: bool, addr: &Multiaddr| {
                if !global_ip_only
                    || multiaddr_to_socketaddr(addr)
                        .map(|addr| is_reachable(addr.ip()))
                        .unwrap_or_default()
                {
                    announce_multiaddrs.push(addr.clone());
                }
            };
        for (key, value) in self.substreams.iter_mut() {
            value.check_timer();

            match value.receive_messages(cx, &mut self.addr_mgr) {
                Ok(Some((session_id, nodes_list))) => {
                    for nodes in nodes_list {
                        self.pending_nodes
                            .push_back((key.clone(), session_id, nodes));
                    }
                }
                Ok(None) => {
                    // stream close
                    self.dead_keys.insert(key.clone());
                }
                Err(err) => {
                    debug!("substream {:?} receive messages error: {:?}", key, err);
                    // remove the substream
                    self.dead_keys.insert(key.clone());
                }
            }

            match value.send_messages(cx) {
                Ok(_) => {}
                Err(err) => {
                    debug!("substream {:?} send messages error: {:?}", key, err);
                    // remove the substream
                    self.dead_keys.insert(key.clone());
                }
            }

            if value.announce {
                if let RemoteAddress::Listen(ref addr) = value.remote_addr {
                    announce_fn(announce_multiaddrs, self.global_ip_only, addr)
                }
                value.announce = false;
                value.last_announce = Some(Instant::now());
            }
        }
    }

    fn remove_dead_stream(&mut self) {
        let mut dead_addr = Vec::default();
        for key in self.dead_keys.drain() {
            if let Some(addr) = self.substreams.remove(&key) {
                dead_addr.push(RawAddr::try_from(addr.remote_addr.into_inner()).unwrap());
            }
        }

        if !dead_addr.is_empty() {
            self.substreams
                .values_mut()
                .for_each(|value| value.addr_known.remove(dead_addr.iter()));
        }
    }

    fn send_messages(&mut self, cx: &mut Context) {
        for (key, value) in self.substreams.iter_mut() {
            let announce_multiaddrs = value.announce_multiaddrs.split_off(0);
            if !announce_multiaddrs.is_empty() {
                let items = announce_multiaddrs
                    .into_iter()
                    .map(|addr| Node {
                        addresses: vec![addr],
                    })
                    .collect::<Vec<_>>();
                let nodes = Nodes {
                    announce: true,
                    items,
                };
                value
                    .pending_messages
                    .push_back(DiscoveryMessage::Nodes(nodes));
            }

            match value.send_messages(cx) {
                Ok(_) => {}
                Err(err) => {
                    debug!("substream {:?} send messages error: {:?}", key, err);
                    // remove the substream
                    self.dead_keys.insert(key.clone());
                }
            }
        }
    }
}

impl<M: AddressManager + Unpin> Stream for Discovery<M> {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        debug!("Discovery.poll()");
        self.recv_substreams(cx);
        self.check_interval(cx);

        let mut announce_multiaddrs = Vec::new();

        self.poll_substreams(cx, &mut announce_multiaddrs);

        self.remove_dead_stream();

        let mut rng = rand::thread_rng();
        let mut remain_keys = self.substreams.keys().cloned().collect::<Vec<_>>();
        debug!("announce_multiaddrs: {:?}", announce_multiaddrs);
        for announce_multiaddr in announce_multiaddrs.into_iter() {
            let announce_addr = RawAddr::try_from(announce_multiaddr.clone()).unwrap();
            remain_keys.shuffle(&mut rng);
            for i in 0..2 {
                if let Some(key) = remain_keys.get(i) {
                    if let Some(value) = self.substreams.get_mut(key) {
                        debug!(
                            ">> send {} to: {:?}, contains: {}",
                            announce_multiaddr,
                            value.remote_addr,
                            value.addr_known.contains(&announce_addr)
                        );
                        if value.announce_multiaddrs.len() < 10
                            && !value.addr_known.contains(&announce_addr)
                        {
                            value.announce_multiaddrs.push(announce_multiaddr.clone());
                            value.addr_known.insert(announce_addr);
                        }
                    }
                }
            }
        }

        self.send_messages(cx);

        match self.pending_nodes.pop_front() {
            Some((_key, session_id, nodes)) => {
                let addrs = nodes
                    .items
                    .into_iter()
                    .flat_map(|node| node.addresses.into_iter())
                    .collect::<Vec<_>>();
                self.addr_mgr.add_new_addrs(session_id, addrs);
                Poll::Ready(Some(()))
            }
            None => Poll::Pending,
        }
    }
}
