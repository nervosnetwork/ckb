use crate::{
    errors::{AddrError, Error},
    peer_store::{
        PeerId, Score, SessionType, ADDR_MAX_FAILURES, ADDR_MAX_RETRIES, ADDR_TIMEOUT_MS,
    },
};
use ipnetwork::IpNetwork;
use p2p::multiaddr::{Multiaddr, Protocol};
use serde::{
    de::{self, Deserializer, MapAccess, SeqAccess, Visitor},
    ser::{SerializeStruct, Serializer},
    Deserialize, Serialize,
};
use std::fmt;
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpPort {
    pub ip: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub connected_addr: Multiaddr,
    pub session_type: SessionType,
    pub ban_time_ms: u64,
    pub last_connected_at_ms: u64,
}

impl PeerInfo {
    pub fn new(
        peer_id: PeerId,
        connected_addr: Multiaddr,
        session_type: SessionType,
        last_connected_at_ms: u64,
    ) -> Self {
        PeerInfo {
            peer_id,
            connected_addr,
            session_type,
            last_connected_at_ms,
            ban_time_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct AddrInfo {
    pub peer_id: PeerId,
    pub ip_port: IpPort,
    pub addr: Multiaddr,
    pub score: Score,
    pub last_connected_at_ms: u64,
    pub last_tried_at_ms: u64,
    pub attempts_count: u32,
    pub random_id_pos: usize,
}

impl Serialize for AddrInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("AddrInfo", 7)?;
        state.serialize_field("peer_id", &self.peer_id.as_bytes())?;
        state.serialize_field("ip_port", &self.ip_port)?;
        state.serialize_field("addr", &self.addr)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("last_connected_at_ms", &self.last_connected_at_ms)?;
        state.serialize_field("last_tried_at_ms", &self.last_tried_at_ms)?;
        state.serialize_field("attempts_count", &self.attempts_count)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for AddrInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            PeerId,
            IpPort,
            Addr,
            Score,
            LastConnectedAtMs,
            LastTriedAtMs,
            AttemptsCount,
        };

        struct AddrInfoVisitor;

        impl<'de> Visitor<'de> for AddrInfoVisitor {
            type Value = AddrInfo;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct AddrInfo")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<AddrInfo, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let peer_id_bytes = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let peer_id = PeerId::from_bytes(peer_id_bytes)
                    .map_err(|err| de::Error::custom(format!("invalid peer_id {:?}", err)))?;
                let ip_port = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let addr = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let score = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                let last_connected_at_ms = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(4, &self))?;
                let last_tried_at_ms = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(5, &self))?;
                let attempts_count = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(6, &self))?;
                Ok(AddrInfo {
                    peer_id,
                    ip_port,
                    addr,
                    score,
                    last_connected_at_ms,
                    last_tried_at_ms,
                    attempts_count,
                    random_id_pos: Default::default(),
                })
            }

            fn visit_map<V>(self, mut map: V) -> Result<AddrInfo, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut peer_id = None;
                let mut ip_port = None;
                let mut addr = None;
                let mut score = None;
                let mut last_connected_at_ms = None;
                let mut last_tried_at_ms = None;
                let mut attempts_count = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::PeerId => {
                            if peer_id.is_some() {
                                return Err(de::Error::duplicate_field("peer_id"));
                            }
                            peer_id =
                                Some(PeerId::from_bytes(map.next_value()?).map_err(|err| {
                                    de::Error::custom(format!("invalid peer_id {:?}", err))
                                })?);
                        }
                        Field::IpPort => {
                            if ip_port.is_some() {
                                return Err(de::Error::duplicate_field("ip_port"));
                            }
                            ip_port = Some(map.next_value()?);
                        }
                        Field::Addr => {
                            if addr.is_some() {
                                return Err(de::Error::duplicate_field("addr"));
                            }
                            addr = Some(map.next_value()?);
                        }
                        Field::Score => {
                            if score.is_some() {
                                return Err(de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        Field::LastConnectedAtMs => {
                            if last_connected_at_ms.is_some() {
                                return Err(de::Error::duplicate_field("last_connected_at_ms"));
                            }
                            last_connected_at_ms = Some(map.next_value()?);
                        }
                        Field::LastTriedAtMs => {
                            if last_tried_at_ms.is_some() {
                                return Err(de::Error::duplicate_field("last_tried_at_ms"));
                            }
                            last_tried_at_ms = Some(map.next_value()?);
                        }
                        Field::AttemptsCount => {
                            if attempts_count.is_some() {
                                return Err(de::Error::duplicate_field("attempts_count"));
                            }
                            attempts_count = Some(map.next_value()?);
                        }
                    }
                }
                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;
                let ip_port = ip_port.ok_or_else(|| de::Error::missing_field("ip_port"))?;
                let addr = addr.ok_or_else(|| de::Error::missing_field("addr"))?;
                let score = score.ok_or_else(|| de::Error::missing_field("score"))?;
                let last_connected_at_ms = last_connected_at_ms
                    .ok_or_else(|| de::Error::missing_field("last_connected_at_ms"))?;
                let last_tried_at_ms =
                    last_tried_at_ms.ok_or_else(|| de::Error::missing_field("last_tried_at_ms"))?;
                let attempts_count =
                    attempts_count.ok_or_else(|| de::Error::missing_field("attempts_count"))?;
                Ok(AddrInfo {
                    peer_id,
                    ip_port,
                    addr,
                    score,
                    last_connected_at_ms,
                    last_tried_at_ms,
                    attempts_count,
                    random_id_pos: Default::default(),
                })
            }
        }

        const FIELDS: &[&str] = &[
            "peer_id",
            "ip_port",
            "addr",
            "score",
            "last_connected_at_ms",
            "last_tried_at_ms",
            "attempts_count",
        ];
        deserializer.deserialize_struct("AddrInfo", FIELDS, AddrInfoVisitor)
    }
}

impl AddrInfo {
    pub fn new(
        peer_id: PeerId,
        ip_port: IpPort,
        addr: Multiaddr,
        last_connected_at_ms: u64,
        score: Score,
    ) -> Self {
        AddrInfo {
            peer_id,
            ip_port,
            addr,
            score,
            last_connected_at_ms,
            last_tried_at_ms: 0,
            attempts_count: 0,
            random_id_pos: 0,
        }
    }

    pub fn ip_port(&self) -> IpPort {
        self.ip_port
    }

    pub fn had_connected(&self, expires_ms: u64) -> bool {
        self.last_connected_at_ms > expires_ms
    }

    pub fn tried_in_last_minute(&self, now_ms: u64) -> bool {
        self.last_tried_at_ms >= now_ms.saturating_sub(60_000)
    }

    pub fn is_terrible(&self, now_ms: u64) -> bool {
        // do not remove addr tried in last minute
        if self.tried_in_last_minute(now_ms) {
            return false;
        }
        // we give up if never connect to this addr
        if self.last_connected_at_ms == 0 && self.attempts_count >= ADDR_MAX_RETRIES {
            return true;
        }
        // consider addr is terrible if failed too many times
        if now_ms.saturating_sub(self.last_connected_at_ms) > ADDR_TIMEOUT_MS
            && (self.attempts_count >= ADDR_MAX_FAILURES)
        {
            return true;
        }
        false
    }

    pub fn mark_tried(&mut self, tried_at_ms: u64) {
        self.last_tried_at_ms = tried_at_ms;
        self.attempts_count = self.attempts_count.saturating_add(1);
    }

    pub fn mark_connected(&mut self, connected_at_ms: u64) {
        self.last_connected_at_ms = connected_at_ms;
        // reset attemps
        self.attempts_count = 0;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BannedAddr {
    pub address: IpNetwork,
    pub ban_until: u64,
    pub ban_reason: String,
    pub created_at: u64,
}

pub fn multiaddr_to_ip_network(multiaddr: &Multiaddr) -> Option<IpNetwork> {
    for addr_component in multiaddr {
        match addr_component {
            Protocol::Ip4(ipv4) => return Some(IpNetwork::V4(ipv4.into())),
            Protocol::Ip6(ipv6) => return Some(IpNetwork::V6(ipv6.into())),
            _ => (),
        }
    }
    None
}

pub fn ip_to_network(ip: IpAddr) -> IpNetwork {
    match ip {
        IpAddr::V4(ipv4) => IpNetwork::V4(ipv4.into()),
        IpAddr::V6(ipv6) => IpNetwork::V6(ipv6.into()),
    }
}

pub trait MultiaddrExt {
    /// extract IP from multiaddr,
    /// return None if multiaddr contains no IP phase
    fn extract_ip_addr(&self) -> Result<IpPort, Error>;
}

impl MultiaddrExt for Multiaddr {
    fn extract_ip_addr(&self) -> Result<IpPort, Error> {
        let mut ip = None;
        let mut port = None;
        for component in self {
            match component {
                Protocol::Ip4(ipv4) => ip = Some(IpAddr::V4(ipv4)),
                Protocol::Ip6(ipv6) => ip = Some(IpAddr::V6(ipv6)),
                Protocol::Tcp(tcp_port) => port = Some(tcp_port),
                _ => (),
            }
        }
        Ok(IpPort {
            ip: ip.ok_or(AddrError::MissingIP)?,
            port: port.ok_or(AddrError::MissingPort)?,
        })
    }
}
