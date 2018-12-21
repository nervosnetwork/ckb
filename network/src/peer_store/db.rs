use super::{Behaviour, Multiaddr, PeerId, PeerStore, ReportResult, Score, ScoringSchema, Status};
use ckb_time::now_ms;
use libp2p::core::Endpoint;
use log::{debug, trace};
use rusqlite::types::{FromSql, ToSql};
use rusqlite::{Connection, NO_PARAMS};

#[derive(Debug)]
pub struct PeerInfo {
    pub id: u32,
    pub peer_id: PeerId,
    pub connected_addr: Multiaddr,
    pub score: Score,
    pub status: Status,
    pub endpoint: Endpoint,
    pub ban_time: u32,
}

pub fn insert_peer_addr(conn: &Connection, peer_info_id: u32, addr: &Multiaddr) -> usize {
    let mut stmt = conn
        .prepare(
            "INSERT INTO peer_addr (peer_info_id, addr)
                     VALUES(:peer_info_id, :addr)",
        )
        .expect("prepare");
    stmt.execute_named(&[
        (":peer_info_id", &peer_info_id),
        (":addr", &addr.to_bytes()),
    ])
    .expect("insert")
}

pub fn insert_peer_info(
    conn: &Connection,
    peer_id: &PeerId,
    connected_addr: &Multiaddr,
    endpoint: Endpoint,
    score: Score,
) {
    let mut stmt = conn.prepare("INSERT INTO peer_info (peer_id, connected_addr, score, status, is_inbound) 
                                    VALUES(:peer_id, :connected_addr, :score, :status, :is_inbound)").unwrap();
    stmt.execute_named(&[
        (":peer_id", &peer_id.as_bytes()),
        (":connected_addr", &connected_addr.to_bytes()),
        (":score", &score),
        (":status", &Into::<String>::into(Status::Unknown)),
        (":is_inbound", &endpoint_to_bool(endpoint)),
    ])
    .expect("insert");
}

pub fn update_peer_info(
    conn: &Connection,
    id: u32,
    connected_addr: &Multiaddr,
    endpoint: Endpoint,
) {
    let mut stmt = conn.prepare("UPDATE peer_info SET connected_addr=:connected_addr, is_inbound=:is_inbound WHERE id=:id").unwrap();
    let _rows = stmt
        .execute_named(&[
            (":connected_addr", &connected_addr.to_bytes()),
            (":is_inbound", &endpoint_to_bool(endpoint)),
            (":id", &id),
        ])
        .expect("update");
}

pub fn get_peer_info_by_peer_id(conn: &Connection, peer_id: &PeerId) -> Option<PeerInfo> {
    let mut stmt = conn.prepare("SELECT id, peer_id, connected_addr, score, status, is_inbound, ban_time FROM peer_info WHERE peer_id=:peer_id LIMIT 1").unwrap();
    let mut rows = stmt
        .query_map_named(&[(":peer_id", &peer_id.as_bytes())], |row| PeerInfo {
            id: row.get(0),
            peer_id: PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
            connected_addr: Multiaddr::from_bytes(row.get(2)).expect("parse multiaddr"),
            score: row.get(3),
            status: Status::from(row.get::<_, String>(4)),
            endpoint: bool_to_endpoint(row.get::<_, bool>(6)),
            ban_time: row.get(7),
        })
        .expect("query");
    rows.next().map(|row| row.expect("query first"))
}

pub fn get_peer_addrs(conn: &Connection, id: u32, count: u32) -> Vec<Multiaddr> {
    let mut stmt = conn
        .prepare("SELECT addr FROM peer_addr WHERE peer_info_id == :id LIMIT :count")
        .expect("prepare");
    let rows = stmt
        .query_map_named(&[(":id", &id), (":count", &count)], |row| {
            Multiaddr::from_bytes(row.get(0)).expect("parse multiaddr")
        })
        .expect("query");
    let mut addrs = Vec::with_capacity(count as usize);
    for row in rows {
        addrs.push(row.expect("extra value from query result"));
    }
    addrs
}

pub fn get_peers_to_attempt(conn: &Connection, count: u32) -> Vec<(PeerId, Multiaddr)> {
    // random select peers
    let mut stmt = conn.prepare("SELECT id, peer_id FROM peer_info WHERE status != :connected_status AND ban_time < strftime('%s','now') ORDER BY RANDOM() LIMIT :count").expect("prepare");
    let rows = stmt
        .query_map_named(
            &[
                (
                    ":connected_status",
                    &Into::<String>::into(Status::Connected) as &ToSql,
                ),
                (":count", &count),
            ],
            |row| {
                (
                    row.get::<_, u32>(0),
                    PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
                )
            },
        )
        .expect("query");
    rows.filter_map(|row| {
        let (id, peer_id) = row.expect("extract value from query result");
        get_peer_addrs(conn, id, 1)
            .pop()
            .map(|addr| (peer_id, addr))
    })
    .collect()
}

pub fn update_peer_info_score(conn: &Connection, id: u32, score: Score) -> usize {
    let mut stmt = conn
        .prepare("UPDATE peer_info score=:score WHERE id=:id")
        .unwrap();
    stmt.execute_named(&[(":score", &score), (":id", &id)])
        .expect("update peer score")
}

pub fn update_peer_info_status(conn: &Connection, id: u32, status: Status) -> usize {
    let mut stmt = conn
        .prepare("UPDATE peer_info status=:status WHERE id=:id")
        .unwrap();
    stmt.execute_named(&[(":status", &Into::<String>::into(status)), (":id", &id)])
        .expect("update peer status")
}

pub fn insert_ban_record(conn: &Connection, ip: Vec<u8>, ban_time: u32) {
    let mut stmt = conn
        .prepare(
            "INSERT OR IGNORE INTO ban_list (ip, ban_time) VALUES(:ip, :ban_time)
                                UPDATE ban_list SET ban_time = :ban_time WHERE ip=:ip",
        )
        .unwrap();
    let _rows = stmt
        .execute_named(&[(":ip", &ip), (":ban_time", &ban_time)])
        .expect("insert");
}

pub fn get_ban_records(conn: &Connection, now: u32) -> Vec<(Vec<u8>, u32)> {
    let mut stmt = conn
        .prepare("SELECT ip, ban_time FROM ban_list WHERE ban_time > :now")
        .unwrap();
    let rows = stmt
        .query_map_named(&[(":now", &now)], |row| {
            (row.get::<_, Vec<u8>>(0), row.get::<_, u32>(1))
        })
        .expect("query");
    rows.map(|row| row.expect("extract value from query"))
        .collect()
}

pub fn clear_expires_banned_ip(conn: &Connection, now: u32) -> Vec<Vec<u8>> {
    let mut stmt = conn
        .prepare("SELECT ip FROM ban_list WHERE ban_time < :now")
        .unwrap();
    let rows = stmt
        .query_map_named(&[(":now", &now)], |row| row.get::<_, Vec<u8>>(0))
        .expect("query");
    let ips = rows
        .map(|row| row.expect("extract value from query"))
        .collect::<Vec<Vec<u8>>>();
    let mut stmt = conn
        .prepare("DELETE FROM ban_list WHERE ban_time < :now")
        .unwrap();
    let _rows = stmt.execute_named(&[(":now", &now)]).expect("delete");
    ips
}

fn endpoint_to_bool(endpoint: Endpoint) -> bool {
    endpoint == Endpoint::Listener
}

fn bool_to_endpoint(is_inbound: bool) -> Endpoint {
    if is_inbound {
        Endpoint::Listener
    } else {
        Endpoint::Dialer
    }
}

impl From<String> for Status {
    fn from(s: String) -> Status {
        match s.as_str() {
            "Connected" => Status::Connected,
            "Disconnected" => Status::Disconnected,
            _ => Status::Unknown,
        }
    }
}

impl Into<String> for Status {
    fn into(self) -> String {
        let s = match self {
            Status::Connected => "Connected",
            Status::Disconnected => "Disconnected",
            Status::Unknown => "Unknown",
        };
        s.to_string()
    }
}
