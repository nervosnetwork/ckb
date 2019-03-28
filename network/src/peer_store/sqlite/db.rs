use crate::network_group::{Group, NetworkGroup};
use crate::peer_store::sqlite::DBError;
use crate::peer_store::{Multiaddr, PeerId, Score, Status};
use crate::SessionType;
use rusqlite::types::ToSql;
use rusqlite::OptionalExtension;
use rusqlite::{Connection, NO_PARAMS};
use std::iter::FromIterator;
use std::time::Duration;

type DBResult<T> = Result<T, DBError>;

pub fn create_tables(conn: &Connection) -> DBResult<()> {
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
    last_connected_at INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql)?;
    let sql = r#"
    CREATE TABLE IF NOT EXISTS peer_addr (
    id INTEGER PRIMARY KEY NOT NULL,
    peer_info_id INTEGER NOT NULL,
    addr BINARY NOT NULL,
    last_connected_at INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_info_id_addr_on_peer_addr ON peer_addr (peer_info_id, addr);
    "#;
    conn.execute_batch(sql)?;
    let sql = r#"
    CREATE TABLE IF NOT EXISTS ban_list (
    id INTEGER PRIMARY KEY NOT NULL,
    ip BINARY UNIQUE NOT NULL,
    ban_time INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql).map_err(Into::into)
}

#[derive(Debug)]
pub struct PeerInfo {
    pub id: u32,
    pub peer_id: PeerId,
    pub connected_addr: Multiaddr,
    pub score: Score,
    pub status: Status,
    pub endpoint: SessionType,
    pub ban_time: Duration,
    pub last_connected_at: Duration,
}

impl PeerInfo {
    pub fn insert(
        conn: &Connection,
        peer_id: &PeerId,
        connected_addr: &Multiaddr,
        endpoint: SessionType,
        score: Score,
        last_connected_at: Duration,
    ) -> DBResult<usize> {
        let network_group = connected_addr.network_group();
        let mut stmt = conn.prepare("INSERT INTO peer_info (peer_id, connected_addr, score, status, endpoint, last_connected_at, network_group, ban_time) 
                                    VALUES(:peer_id, :connected_addr, :score, :status, :endpoint, :last_connected_at, :network_group, 0)").expect("prepare");
        stmt.execute_named(&[
            (":peer_id", &peer_id.as_bytes()),
            (":connected_addr", &connected_addr.to_bytes()),
            (":score", &score),
            (":status", &status_to_u8(Status::Unknown)),
            (":endpoint", &endpoint_to_bool(endpoint)),
            (":last_connected_at", &duration_to_secs(last_connected_at)),
            (":network_group", &network_group_to_bytes(&network_group)),
        ])
        .map_err(Into::into)
    }

    pub fn get_or_insert(
        conn: &Connection,
        peer_id: &PeerId,
        addr: &Multiaddr,
        session_type: SessionType,
        score: Score,
        last_connected_at: Duration,
    ) -> DBResult<Option<PeerInfo>> {
        match Self::get_by_peer_id(conn, peer_id)? {
            Some(peer) => Ok(Some(peer)),
            None => {
                Self::insert(conn, peer_id, addr, session_type, score, last_connected_at)?;
                Self::get_by_peer_id(conn, peer_id)
            }
        }
    }

    pub fn update(
        conn: &Connection,
        peer_id: &PeerId,
        connected_addr: &Multiaddr,
        endpoint: SessionType,
        last_connected_at: Duration,
    ) -> DBResult<usize> {
        let mut stmt = conn
            .prepare(
                "UPDATE peer_info SET connected_addr=:connected_addr, endpoint=:endpoint, last_connected_at=:last_connected_at WHERE peer_id=:peer_id",
                )
            .expect("prepare");
        stmt.execute_named(&[
            (":connected_addr", &connected_addr.to_bytes()),
            (":endpoint", &endpoint_to_bool(endpoint)),
            (":last_connected_at", &duration_to_secs(last_connected_at)),
            (":peer_id", &peer_id.as_bytes()),
        ])
        .map_err(Into::into)
    }

    pub fn delete(conn: &Connection, id: u32) -> DBResult<usize> {
        conn.execute("DELETE FROM peer_info WHERE id=?1", &[id])
            .map_err(Into::into)
    }

    pub fn get_by_peer_id(conn: &Connection, peer_id: &PeerId) -> DBResult<Option<PeerInfo>> {
        conn.query_row("SELECT id, peer_id, connected_addr, score, status, endpoint, ban_time, last_connected_at FROM peer_info WHERE peer_id=?1 LIMIT 1", &[&peer_id.as_bytes()], |row| PeerInfo {
            id: row.get(0),
            peer_id: PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
            connected_addr: Multiaddr::from_bytes(row.get(2)).expect("parse multiaddr"),
            score: row.get(3),
            status: u8_to_status(row.get::<_, u8>(4)),
            endpoint: bool_to_endpoint(row.get::<_, bool>(5)),
            ban_time: secs_to_duration(row.get(6)),
            last_connected_at: secs_to_duration(row.get(7)),
        }).optional().map_err(Into::into)
    }

    pub fn update_score(conn: &Connection, id: u32, score: Score) -> DBResult<usize> {
        let mut stmt = conn.prepare("UPDATE peer_info SET score=:score WHERE id=:id")?;
        stmt.execute_named(&[(":score", &score), (":id", &id)])
            .map_err(Into::into)
    }

    pub fn update_status(conn: &Connection, id: u32, status: Status) -> DBResult<usize> {
        let mut stmt = conn.prepare("UPDATE peer_info SET status=:status WHERE id=:id")?;
        stmt.execute_named(&[(":status", &status_to_u8(status)), (":id", &id)])
            .map_err(Into::into)
    }

    pub fn largest_network_group(conn: &Connection) -> DBResult<Vec<PeerInfo>> {
        let (network_group, _group_peers_count) = conn
            .query_row::<(Vec<u8>, u32), _, _>("SELECT network_group, COUNT(network_group) AS network_group_count FROM peer_info
                                               GROUP BY network_group ORDER BY network_group_count DESC LIMIT 1",
                                               NO_PARAMS, |r| (r.get(0), r.get(1)))?;
        let mut stmt = conn.prepare("SELECT id, peer_id, connected_addr, score, status, endpoint, ban_time, last_connected_at FROM peer_info
                                    WHERE network_group=:network_group")?;
        let rows = stmt.query_map_named(&[(":network_group", &network_group)], |row| PeerInfo {
            id: row.get(0),
            peer_id: PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
            connected_addr: Multiaddr::from_bytes(row.get(2)).expect("parse multiaddr"),
            score: row.get(3),
            status: u8_to_status(row.get::<_, u8>(4)),
            endpoint: bool_to_endpoint(row.get::<_, bool>(5)),
            ban_time: secs_to_duration(row.get(6)),
            last_connected_at: secs_to_duration(row.get(7)),
        })?;
        Result::from_iter(rows).map_err(Into::into)
    }

    pub fn count(conn: &Connection) -> DBResult<u32> {
        conn.query_row::<u32, _, _>("SELECT COUNT(*) FROM peer_info", NO_PARAMS, |r| r.get(0))
            .map_err(Into::into)
    }
}

pub struct PeerAddr;

impl PeerAddr {
    pub fn insert(
        conn: &Connection,
        peer_info_id: u32,
        addr: &Multiaddr,
        last_connected_at: Duration,
    ) -> DBResult<usize> {
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO peer_addr (peer_info_id, addr, last_connected_at)
                     VALUES(:peer_info_id, :addr, :last_connected_at)",
        )?;
        stmt.execute_named(&[
            (":peer_info_id", &peer_info_id),
            (":addr", &addr.to_bytes()),
            (":last_connected_at", &duration_to_secs(last_connected_at)),
        ])
        .map_err(Into::into)
    }

    pub fn update_connected_at(
        conn: &Connection,
        peer_info_id: u32,
        addr: Multiaddr,
        last_connected_at: Duration,
    ) -> DBResult<usize> {
        let mut stmt = conn.prepare(
            "UPDATE peer_addr SET last_connected_at=:last_connected_at
                    WHERE peer_info_id=:peer_info_id AND addr=:addr",
        )?;
        stmt.execute_named(&[
            (":peer_info_id", &peer_info_id),
            (":addr", &addr.to_bytes()),
            (":last_connected_at", &duration_to_secs(last_connected_at)),
        ])
        .map_err(Into::into)
    }

    pub fn get_addrs(conn: &Connection, id: u32, count: u32) -> DBResult<Vec<Multiaddr>> {
        let mut stmt = conn.prepare(
            "SELECT addr FROM peer_addr WHERE peer_info_id == :id 
                         ORDER BY last_connected_at DESC LIMIT :count",
        )?;
        let rows = stmt.query_map_named(&[(":id", &id), (":count", &count)], |row| {
            Multiaddr::from_bytes(row.get(0)).expect("parse multiaddr")
        })?;
        Result::from_iter(rows).map_err(Into::into)
    }

    pub fn delete_by_peer_id(conn: &Connection, id: u32) -> DBResult<usize> {
        conn.execute("DELETE FROM peer_addr WHERE peer_info_id=?1", &[id])
            .map_err(Into::into)
    }
}

pub fn get_random_peers(
    conn: &Connection,
    count: u32,
    expired_at: Duration,
) -> DBResult<Vec<(PeerId, Multiaddr)>> {
    // random select peers that we have connect to recently.
    let mut stmt = conn.prepare(
        "SELECT id, peer_id FROM peer_info 
                                WHERE ban_time < strftime('%s','now') 
                                AND last_connected_at > :time 
                                ORDER BY RANDOM() LIMIT :count",
    )?;
    let rows = stmt.query_map_named(
        &[(":count", &count), (":time", &duration_to_secs(expired_at))],
        |row| {
            (
                row.get::<_, u32>(0),
                PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
            )
        },
    )?;

    let mut peers = Vec::with_capacity(count as usize);
    for row in rows {
        let (id, peer_id) = row?;
        let mut addrs = PeerAddr::get_addrs(conn, id, 1)?;
        if let Some(addr) = addrs.pop() {
            peers.push((peer_id, addr));
        }
    }
    Ok(peers)
}

pub fn get_peers_to_attempt(conn: &Connection, count: u32) -> DBResult<Vec<(PeerId, Multiaddr)>> {
    // random select peers
    let mut stmt = conn.prepare(
        "SELECT id, peer_id FROM peer_info 
                                WHERE status != :connected_status 
                                AND ban_time < strftime('%s','now') 
                                ORDER BY RANDOM() LIMIT :count",
    )?;
    let rows = stmt.query_map_named(
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
    )?;

    let mut peers = Vec::with_capacity(count as usize);
    for row in rows {
        let (id, peer_id) = row?;
        let mut addrs = PeerAddr::get_addrs(conn, id, 1)?;
        if let Some(addr) = addrs.pop() {
            peers.push((peer_id, addr));
        }
    }
    Ok(peers)
}

pub fn get_peers_to_feeler(
    conn: &Connection,
    count: u32,
    expired_at: Duration,
) -> DBResult<Vec<(PeerId, Multiaddr)>> {
    // random select peers
    let mut stmt = conn.prepare(
        "SELECT id, peer_id FROM peer_info 
                                WHERE status != :connected_status 
                                AND last_connected_at < :time 
                                AND ban_time < strftime('%s','now') 
                                ORDER BY RANDOM() LIMIT :count",
    )?;
    let rows = stmt.query_map_named(
        &[
            (
                ":connected_status",
                &status_to_u8(Status::Connected) as &ToSql,
            ),
            (":time", &duration_to_secs(expired_at)),
            (":count", &count),
        ],
        |row| {
            (
                row.get::<_, u32>(0),
                PeerId::from_bytes(row.get(1)).expect("parse peer_id"),
            )
        },
    )?;

    let mut peers = Vec::with_capacity(count as usize);
    for row in rows {
        let (id, peer_id) = row?;
        let mut addrs = PeerAddr::get_addrs(conn, id, 1)?;
        if let Some(addr) = addrs.pop() {
            peers.push((peer_id, addr));
        }
    }
    Ok(peers)
}

pub fn insert_ban_record(conn: &Connection, ip: &[u8], ban_time: Duration) -> DBResult<usize> {
    let mut stmt =
        conn.prepare("INSERT OR REPLACE INTO ban_list (ip, ban_time) VALUES(:ip, :ban_time);")?;
    stmt.execute_named(&[(":ip", &ip), (":ban_time", &duration_to_secs(ban_time))])
        .map_err(Into::into)
}

pub fn get_ban_records(conn: &Connection, now: Duration) -> DBResult<Vec<(Vec<u8>, Duration)>> {
    let mut stmt = conn.prepare("SELECT ip, ban_time FROM ban_list WHERE ban_time > :now")?;
    let rows = stmt.query_map_named(&[(":now", &duration_to_secs(now))], |row| {
        (row.get::<_, Vec<u8>>(0), secs_to_duration(row.get(1)))
    })?;
    Result::from_iter(rows).map_err(Into::into)
}

pub fn clear_expires_banned_ip(conn: &Connection, now: Duration) -> DBResult<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT ip FROM ban_list WHERE ban_time < :now")?;
    let rows = stmt.query_map_named(&[(":now", &duration_to_secs(now))], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;
    let mut stmt = conn.prepare("DELETE FROM ban_list WHERE ban_time < :now")?;
    stmt.execute_named(&[(":now", &duration_to_secs(now))])?;
    Result::from_iter(rows).map_err(Into::into)
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

fn endpoint_to_bool(endpoint: SessionType) -> bool {
    endpoint == SessionType::Server
}

fn bool_to_endpoint(is_inbound: bool) -> SessionType {
    if is_inbound {
        SessionType::Server
    } else {
        SessionType::Client
    }
}

fn network_group_to_bytes(network_group: &Group) -> Vec<u8> {
    format!("{:?}", network_group).into_bytes()
}
