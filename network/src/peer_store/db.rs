use super::{Multiaddr, PeerId, Score, Status};
use crate::network_group::{Group, NetworkGroup};
use libp2p::core::Endpoint;
use rusqlite::types::ToSql;
use rusqlite::{Connection, NO_PARAMS};
use std::time::Duration;

pub fn create_tables(conn: &Connection) {
    let sql = r#"
    CREATE TABLE IF NOT EXISTS peer_info (
    id INTEGER PRIMARY KEY NOT NULL,
    peer_id BINARY UNIQUE NOT NULL,
    connected_addr BINARY NOT NULL,
    network_group BINARY NOT NULL,
    score INTEGER NOT NULL,
    status INTEGER NOT NULL,
    endpoint INTEGER NOT NULL,
    ban_time INTEGER NOT NULL,
    connected_time INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql).expect("crate peer_info table");
    let sql = r#"
    CREATE TABLE IF NOT EXISTS peer_addr (
    id INTEGER PRIMARY KEY NOT NULL,
    peer_info_id INTEGER NOT NULL,
    addr BINARY NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_info_id_addr_on_peer_addr ON peer_addr (peer_info_id, addr);
    "#;
    conn.execute_batch(sql).expect("create peer_addr table");
    let sql = r#"
    CREATE TABLE IF NOT EXISTS ban_list (
    id INTEGER PRIMARY KEY NOT NULL,
    ip BINARY UNIQUE NOT NULL,
    ban_time INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql).expect("create ban_list table");
}

#[derive(Debug)]
pub struct PeerInfo {
    pub id: u32,
    pub peer_id: PeerId,
    pub connected_addr: Multiaddr,
    pub score: Score,
    pub status: Status,
    pub endpoint: Endpoint,
    pub ban_time: Duration,
    pub connected_time: Duration,
}

impl PeerInfo {
    pub fn insert(
        conn: &Connection,
        peer_id: &PeerId,
        connected_addr: &Multiaddr,
        endpoint: Endpoint,
        score: Score,
        connected_time: Duration,
    ) {
        let network_group = connected_addr.network_group();
        let mut stmt = conn.prepare("INSERT INTO peer_info (peer_id, connected_addr, score, status, endpoint, connected_time, network_group, ban_time) 
                                    VALUES(:peer_id, :connected_addr, :score, :status, :endpoint, :connected_time, :network_group, 0)").expect("prepare");
        stmt.execute_named(&[
            (":peer_id", &peer_id.as_bytes()),
            (":connected_addr", &connected_addr.to_bytes()),
            (":score", &score),
            (":status", &status_to_u8(Status::Unknown)),
            (":endpoint", &endpoint_to_bool(endpoint)),
            (":connected_time", &duration_to_secs(connected_time)),
            (":network_group", &network_group_to_bytes(&network_group)),
        ])
        .expect("insert");
    }
    pub fn update(
        conn: &Connection,
        id: u32,
        connected_addr: &Multiaddr,
        endpoint: Endpoint,
        connected_time: Duration,
    ) {
        let mut stmt = conn
            .prepare(
                "UPDATE peer_info SET connected_addr=:connected_addr, endpoint=:endpoint, connected_time=:connected_time WHERE id=:id",
                )
            .expect("prepare");
        let _rows = stmt
            .execute_named(&[
                (":connected_addr", &connected_addr.to_bytes()),
                (":endpoint", &endpoint_to_bool(endpoint)),
                (":connected_time", &duration_to_secs(connected_time)),
                (":id", &id),
            ])
            .expect("update");
    }

    pub fn delete(conn: &mut Connection, id: u32) {
        let tx = conn.transaction().expect("start db transaction");
        tx.execute("DELETE FROM peer_info WHERE id=?1", &[id])
            .expect("prepare");
        tx.execute("DELETE FROM peer_addr WHERE peer_info_id=?1", &[id])
            .expect("prepare");
        tx.commit().expect("commit");
    }

    pub fn get_by_peer_id(conn: &Connection, peer_id: &PeerId) -> Option<PeerInfo> {
        let mut stmt = conn.prepare("SELECT id, peer_id, connected_addr, score, status, endpoint, ban_time, connected_time FROM peer_info WHERE peer_id=:peer_id LIMIT 1").expect("prepare");
        let mut rows = stmt
            .query_map_named(&[(":peer_id", &peer_id.as_bytes())], |row| PeerInfo {
                id: row.get(0),
                peer_id: PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
                connected_addr: Multiaddr::from_bytes(row.get(2)).expect("parse multiaddr"),
                score: row.get(3),
                status: u8_to_status(row.get::<_, u8>(4)),
                endpoint: bool_to_endpoint(row.get::<_, bool>(5)),
                ban_time: secs_to_duration(row.get(6)),
                connected_time: secs_to_duration(row.get(7)),
            })
            .expect("query");
        rows.next().map(|row| row.expect("query first"))
    }

    pub fn update_score(conn: &Connection, id: u32, score: Score) -> usize {
        let mut stmt = conn
            .prepare("UPDATE peer_info SET score=:score WHERE id=:id")
            .expect("prepare");
        stmt.execute_named(&[(":score", &score), (":id", &id)])
            .expect("update peer score")
    }

    pub fn update_status(conn: &Connection, id: u32, status: Status) -> usize {
        let mut stmt = conn
            .prepare("UPDATE peer_info SET status=:status WHERE id=:id")
            .expect("prepare");
        stmt.execute_named(&[(":status", &status_to_u8(status)), (":id", &id)])
            .expect("update peer status")
    }

    pub fn largest_network_group(conn: &Connection) -> Vec<PeerInfo> {
        let (network_group, _group_peers_count) = conn.query_row::<(Vec<u8>, u32), _, _>("SELECT network_group, COUNT(network_group) AS network_group_count FROM peer_info GROUP BY network_group ORDER BY network_group_count DESC LIMIT 1", NO_PARAMS, |r| (r.get(0), r.get(1))).expect("query count");
        let mut stmt = conn.prepare("SELECT id, peer_id, connected_addr, score, status, endpoint, ban_time, connected_time FROM peer_info WHERE network_group=:network_group").expect("prepare");
        let rows = stmt
            .query_map_named(&[(":network_group", &network_group)], |row| PeerInfo {
                id: row.get(0),
                peer_id: PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
                connected_addr: Multiaddr::from_bytes(row.get(2)).expect("parse multiaddr"),
                score: row.get(3),
                status: u8_to_status(row.get::<_, u8>(4)),
                endpoint: bool_to_endpoint(row.get::<_, bool>(5)),
                ban_time: secs_to_duration(row.get(6)),
                connected_time: secs_to_duration(row.get(7)),
            })
            .expect("query");
        rows.map(|row| row.expect("extra value from query result"))
            .collect()
    }

    pub fn count(conn: &Connection) -> u32 {
        conn.query_row::<u32, _, _>("SELECT COUNT(*) FROM peer_info", NO_PARAMS, |r| r.get(0))
            .expect("query count")
    }
}

pub struct PeerAddr;

impl PeerAddr {
    pub fn insert(conn: &Connection, peer_info_id: u32, addr: &Multiaddr) -> usize {
        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO peer_addr (peer_info_id, addr)
                     VALUES(:peer_info_id, :addr)",
            )
            .expect("prepare");
        stmt.execute_named(&[
            (":peer_info_id", &peer_info_id),
            (":addr", &addr.to_bytes()),
        ])
        .expect("insert")
    }

    pub fn get_addrs(conn: &Connection, id: u32, count: u32) -> Vec<Multiaddr> {
        let mut stmt = conn
            .prepare("SELECT addr FROM peer_addr WHERE peer_info_id == :id LIMIT :count")
            .expect("prepare");
        let rows = stmt
            .query_map_named(&[(":id", &id), (":count", &count)], |row| {
                Multiaddr::from_bytes(row.get(0)).expect("parse multiaddr")
            })
            .expect("query");
        rows.map(|row| row.expect("extra value from query result"))
            .collect()
    }
}

pub fn get_peers_to_attempt(conn: &Connection, count: u32) -> Vec<(PeerId, Multiaddr)> {
    // random select peers
    let mut stmt = conn.prepare("SELECT id, peer_id FROM peer_info WHERE status != :connected_status AND ban_time < strftime('%s','now') ORDER BY RANDOM() LIMIT :count").expect("prepare");
    let rows = stmt
        .query_map_named(
            &[
                (
                    ":connected_status",
                    &status_to_u8(Status::Connected) as &ToSql,
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
        PeerAddr::get_addrs(conn, id, 1)
            .pop()
            .map(|addr| (peer_id, addr))
    })
    .collect()
}

pub fn insert_ban_record(conn: &Connection, ip: &[u8], ban_time: Duration) {
    let mut stmt = conn
        .prepare("INSERT OR REPLACE INTO ban_list (ip, ban_time) VALUES(:ip, :ban_time);")
        .expect("prepare");
    let _rows = stmt
        .execute_named(&[(":ip", &ip), (":ban_time", &duration_to_secs(ban_time))])
        .expect("insert");
}

pub fn get_ban_records(conn: &Connection, now: Duration) -> Vec<(Vec<u8>, Duration)> {
    let mut stmt = conn
        .prepare("SELECT ip, ban_time FROM ban_list WHERE ban_time > :now")
        .expect("prepare");
    let rows = stmt
        .query_map_named(&[(":now", &duration_to_secs(now))], |row| {
            (row.get::<_, Vec<u8>>(0), secs_to_duration(row.get(1)))
        })
        .expect("query");
    rows.map(|row| row.expect("extract value from query"))
        .collect()
}

pub fn clear_expires_banned_ip(conn: &Connection, now: Duration) -> Vec<Vec<u8>> {
    let mut stmt = conn
        .prepare("SELECT ip FROM ban_list WHERE ban_time < :now")
        .expect("prepare");
    let rows = stmt
        .query_map_named(&[(":now", &duration_to_secs(now))], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .expect("query");
    let ips = rows
        .map(|row| row.expect("extract value from query"))
        .collect::<Vec<Vec<u8>>>();
    let mut stmt = conn
        .prepare("DELETE FROM ban_list WHERE ban_time < :now")
        .expect("prepare");
    let _rows = stmt
        .execute_named(&[(":now", &duration_to_secs(now))])
        .expect("delete");
    ips
}

fn status_to_u8(status: Status) -> u8 {
    status as u8
}

fn u8_to_status(i: u8) -> Status {
    Status::from(i)
}

fn secs_to_duration(secs: u32) -> Duration {
    Duration::from_secs(secs.into())
}

fn duration_to_secs(duration: Duration) -> u32 {
    duration.as_secs() as u32
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

fn network_group_to_bytes(network_group: &Group) -> Vec<u8> {
    format!("{:?}", network_group).into_bytes()
}
