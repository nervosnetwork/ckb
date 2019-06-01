use crate::network_group::{Group, NetworkGroup};
use crate::peer_store::sqlite::DBError;
use crate::peer_store::types::{PeerAddr, PeerInfo};
use crate::peer_store::{Multiaddr, PeerId, Protocol, Score, Status};
use crate::SessionType;
use rusqlite::types::ToSql;
use rusqlite::OptionalExtension;
use rusqlite::{Connection, NO_PARAMS};
use std::convert::TryFrom;
use std::iter::FromIterator;

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
    ban_time_secs INTEGER NOT NULL,
    last_connected_at_secs INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql)?;
    let sql = r#"
    CREATE TABLE IF NOT EXISTS peer_addr (
    id INTEGER PRIMARY KEY NOT NULL,
    peer_id BINARY NOT NULL,
    addr BINARY NOT NULL,
    last_connected_at_secs INTEGER NOT NULL,
    last_tried_at_secs INTEGER NOT NULL,
    attempts_count INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_id_addr_on_peer_addr ON peer_addr (peer_id, addr);
    "#;
    conn.execute_batch(sql)?;
    let sql = r#"
    CREATE TABLE IF NOT EXISTS ban_list (
    id INTEGER PRIMARY KEY NOT NULL,
    ip BINARY UNIQUE NOT NULL,
    ban_time_secs INTEGER NOT NULL
    );
    "#;
    conn.execute_batch(sql).map_err(Into::into)
}

pub struct PeerInfoDB;

impl PeerInfoDB {
    pub fn insert_or_update(conn: &Connection, peer: &PeerInfo) -> DBResult<usize> {
        let network_group = peer.connected_addr.network_group();
        let mut stmt = conn.prepare("INSERT OR REPLACE INTO peer_info (peer_id, connected_addr, score, status, endpoint, last_connected_at_secs, network_group, ban_time_secs) 
                                    VALUES(:peer_id, :connected_addr, :score, :status, :endpoint, :last_connected_at_secs, :network_group, 0)").expect("prepare");
        stmt.execute_named(&[
            (":peer_id", &peer.peer_id.as_bytes()),
            (":connected_addr", &peer.connected_addr.as_ref()),
            (":score", &peer.score),
            (":status", &status_to_u8(peer.status)),
            (":endpoint", &endpoint_to_bool(peer.session_type)),
            (
                ":last_connected_at_secs",
                &millis_to_secs(peer.last_connected_at_ms),
            ),
            (":network_group", &network_group_to_bytes(&network_group)),
        ])
        .map_err(Into::into)
    }

    pub fn get_or_insert(conn: &Connection, peer: PeerInfo) -> DBResult<Option<PeerInfo>> {
        match Self::get_by_peer_id(conn, &peer.peer_id)? {
            Some(peer) => Ok(Some(peer)),
            None => {
                Self::insert_or_update(conn, &peer)?;
                Ok(Some(peer))
            }
        }
    }

    pub fn update(
        conn: &Connection,
        peer_id: &PeerId,
        connected_addr: &Multiaddr,
        endpoint: SessionType,
        last_connected_at_ms: u64,
    ) -> DBResult<usize> {
        let mut stmt = conn
            .prepare(
                "UPDATE peer_info SET connected_addr=:connected_addr, endpoint=:endpoint, last_connected_at_secs=:last_connected_at_secs WHERE peer_id=:peer_id",
                )
            .expect("prepare");
        stmt.execute_named(&[
            (":connected_addr", &connected_addr.as_ref()),
            (":endpoint", &endpoint_to_bool(endpoint)),
            (
                ":last_connected_at_secs",
                &millis_to_secs(last_connected_at_ms),
            ),
            (":peer_id", &peer_id.as_bytes()),
        ])
        .map_err(Into::into)
    }

    pub fn delete(conn: &Connection, peer_id: &PeerId) -> DBResult<usize> {
        conn.execute(
            "DELETE FROM peer_info WHERE peer_id=?1",
            &[peer_id.as_bytes()],
        )
        .map_err(Into::into)
    }

    pub fn get_by_peer_id(conn: &Connection, peer_id: &PeerId) -> DBResult<Option<PeerInfo>> {
        conn.query_row("SELECT peer_id, connected_addr, score, status, endpoint, ban_time_secs, last_connected_at_secs FROM peer_info WHERE peer_id=?1 LIMIT 1", &[&peer_id.as_bytes()], |row| Ok(PeerInfo {
            peer_id: PeerId::from_bytes(row.get(0)?).expect("parse peer_id"),
            connected_addr: Multiaddr::try_from(row.get::<_, Vec<u8>>(1)?).expect("parse multiaddr"),
            score: row.get(2)?,
            status: u8_to_status(row.get::<_, u8>(3)?),
            session_type: bool_to_endpoint(row.get::<_, bool>(4)?),
            ban_time_ms: secs_to_millis(row.get(5)?),
            last_connected_at_ms: secs_to_millis(row.get(6)?),
        })).optional().map_err(Into::into)
    }

    pub fn update_score(conn: &Connection, peer_id: &PeerId, score: Score) -> DBResult<usize> {
        let mut stmt = conn.prepare("UPDATE peer_info SET score=:score WHERE peer_id=:peer_id")?;
        stmt.execute_named(&[(":score", &score), (":peer_id", &peer_id.as_bytes())])
            .map_err(Into::into)
    }

    pub fn update_status(conn: &Connection, peer_id: &PeerId, status: Status) -> DBResult<usize> {
        let mut stmt =
            conn.prepare("UPDATE peer_info SET status=:status WHERE peer_id=:peer_id")?;
        stmt.execute_named(&[
            (":status", &status_to_u8(status) as &ToSql),
            (":peer_id", &peer_id.as_bytes()),
        ])
        .map_err(Into::into)
    }

    pub fn reset_status(conn: &Connection) -> DBResult<usize> {
        let mut stmt = conn.prepare("UPDATE peer_info SET status=:status WHERE status!=:status")?;
        stmt.execute_named(&[(":status", &status_to_u8(Status::Disconnected))])
            .map_err(Into::into)
    }

    pub fn largest_network_group(conn: &Connection) -> DBResult<Vec<PeerInfo>> {
        let (network_group, _group_peers_count) = conn
            .query_row::<(Vec<u8>, u32), _, _>("SELECT network_group, COUNT(network_group) AS network_group_count FROM peer_info
                                               GROUP BY network_group ORDER BY network_group_count DESC LIMIT 1",
                                               NO_PARAMS, |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut stmt = conn.prepare("SELECT peer_id, connected_addr, score, status, endpoint, ban_time_secs, last_connected_at_secs FROM peer_info
                                    WHERE network_group=:network_group")?;
        let rows = stmt.query_map_named(&[(":network_group", &network_group)], |row| {
            Ok(PeerInfo {
                peer_id: PeerId::from_bytes(row.get(0)?).expect("parse peer_id"),
                connected_addr: Multiaddr::try_from(row.get::<_, Vec<u8>>(1)?)
                    .expect("parse multiaddr"),
                score: row.get(2)?,
                status: u8_to_status(row.get::<_, u8>(3)?),
                session_type: bool_to_endpoint(row.get::<_, bool>(4)?),
                ban_time_ms: secs_to_millis(row.get(5)?),
                last_connected_at_ms: secs_to_millis(row.get(6)?),
            })
        })?;
        Result::from_iter(rows).map_err(Into::into)
    }

    pub fn count(conn: &Connection) -> DBResult<u32> {
        conn.query_row::<u32, _, _>("SELECT COUNT(*) FROM peer_info", NO_PARAMS, |r| r.get(0))
            .map_err(Into::into)
    }
}

pub struct PeerAddrDB;

impl PeerAddrDB {
    pub fn count(conn: &Connection, peer_id: &PeerId) -> DBResult<u32> {
        conn.query_row::<u32, _, _>(
            "SELECT COUNT(*) FROM peer_addr WHERE peer_id=?1",
            &[&peer_id.as_bytes()],
            |r| r.get(0),
        )
        .map_err(Into::into)
    }

    pub fn get(
        conn: &Connection,
        peer_id: &PeerId,
        addr: &Multiaddr,
    ) -> DBResult<Option<PeerAddr>> {
        conn.query_row("SELECT peer_id, addr, last_connected_at_secs, last_tried_at_secs, last_tried_at_secs, attempts_count FROM peer_addr WHERE peer_id=?1 AND addr=?2 LIMIT 1", &[&peer_id.as_bytes(), &addr.as_ref()], |row| Ok(PeerAddr {
            peer_id: PeerId::from_bytes(row.get(0)?).expect("parse peer_id"),
            addr: Multiaddr::try_from(row.get::<_, Vec<u8>>(1)?).expect("parse multiaddr"),
            last_connected_at_ms: secs_to_millis(row.get(2)?),
            last_tried_at_ms: secs_to_millis(row.get(3)?),
            attempts_count: row.get(4)?,
        })).optional().map_err(Into::into)
    }
    pub fn insert_or_update(conn: &Connection, peer_addr: &PeerAddr) -> DBResult<usize> {
        let addr = peer_addr
            .addr
            .into_iter()
            .filter(|proto| match proto {
                Protocol::P2p(_) => false,
                _ => true,
            })
            .collect::<Multiaddr>();

        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO peer_addr (peer_id, addr, last_connected_at_secs, last_tried_at_secs, attempts_count)
                     VALUES(:peer_id, :addr, :last_connected_at_secs, :last_tried_at_secs, :attempts_count)",
        )?;
        stmt.execute_named(&[
            (":peer_id", &peer_addr.peer_id.as_bytes()),
            (":addr", &addr.as_ref()),
            (
                ":last_connected_at_secs",
                &millis_to_secs(peer_addr.last_connected_at_ms),
            ),
            (
                ":last_tried_at_secs",
                &millis_to_secs(peer_addr.last_tried_at_ms),
            ),
            (":attempts_count", &peer_addr.attempts_count),
        ])
        .map_err(Into::into)
    }

    pub fn get_addrs(conn: &Connection, peer_id: &PeerId, count: u32) -> DBResult<Vec<PeerAddr>> {
        let mut stmt = conn.prepare(
            "SELECT peer_id, addr, last_connected_at_secs, last_tried_at_secs, attempts_count FROM peer_addr 
            WHERE peer_id == :peer_id LIMIT :count",
        )?;
        let rows = stmt.query_map_named(
            &[(":peer_id", &peer_id.as_bytes()), (":count", &count)],
            |row| {
                Ok(PeerAddr {
                    peer_id: PeerId::from_bytes(row.get(0)?).expect("parse peer_id"),
                    addr: Multiaddr::try_from(row.get::<_, Vec<u8>>(1)?).expect("parse multiaddr"),
                    last_connected_at_ms: secs_to_millis(row.get(2)?),
                    last_tried_at_ms: secs_to_millis(row.get(3)?),
                    attempts_count: row.get(4)?,
                })
            },
        )?;
        Result::from_iter(rows).map_err(Into::into)
    }

    pub fn delete(conn: &Connection, peer_id: &PeerId, addr: &Multiaddr) -> DBResult<usize> {
        conn.execute(
            "DELETE FROM peer_addr WHERE peer_id=?1 AND addr=?2",
            &[&peer_id.as_bytes(), &addr.as_ref()],
        )
        .map_err(Into::into)
    }

    pub fn delete_by_peer_id(conn: &Connection, peer_id: &PeerId) -> DBResult<usize> {
        conn.execute(
            "DELETE FROM peer_addr WHERE peer_id=?1",
            &[peer_id.as_bytes()],
        )
        .map_err(Into::into)
    }
}

pub fn get_random_peers(
    conn: &Connection,
    count: u32,
    expired_at_ms: u64,
) -> DBResult<Vec<PeerId>> {
    // random select peers that we have connect to recently.
    let mut stmt = conn.prepare(
        "SELECT peer_id FROM peer_info 
                                WHERE ban_time_secs < strftime('%s','now') 
                                AND last_connected_at_secs > :time 
                                ORDER BY RANDOM() LIMIT :count",
    )?;
    let rows = stmt.query_map_named(
        &[
            (":count", &count),
            (":time", &millis_to_secs(expired_at_ms)),
        ],
        |row| Ok(PeerId::from_bytes(row.get(0)?).expect("parse peer_id")),
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_peers_to_attempt(conn: &Connection, count: u32) -> DBResult<Vec<PeerId>> {
    // random select peers
    let mut stmt = conn.prepare(
        "SELECT peer_id FROM peer_info 
                                WHERE status != :connected_status 
                                AND ban_time_secs < strftime('%s','now') 
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
        |row| Ok(PeerId::from_bytes(row.get(0)?).expect("parse peer_id")),
    )?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_peers_to_feeler(
    conn: &Connection,
    count: u32,
    expired_at_ms: u64,
) -> DBResult<Vec<PeerId>> {
    // random select peers
    let mut stmt = conn.prepare(
        "SELECT peer_id FROM peer_info 
                                WHERE status != :connected_status 
                                AND last_connected_at_secs < :time 
                                AND ban_time_secs < strftime('%s','now') 
                                ORDER BY RANDOM() LIMIT :count",
    )?;
    let rows = stmt.query_map_named(
        &[
            (
                ":connected_status",
                &status_to_u8(Status::Connected) as &ToSql,
            ),
            (":time", &millis_to_secs(expired_at_ms)),
            (":count", &count),
        ],
        |row| Ok(PeerId::from_bytes(row.get(0)?).expect("parse peer_id")),
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn insert_ban_record(conn: &Connection, ip: &[u8], ban_time_ms: u64) -> DBResult<usize> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO ban_list (ip, ban_time_secs) VALUES(:ip, :ban_time_secs);",
    )?;
    stmt.execute_named(&[
        (":ip", &ip),
        (":ban_time_secs", &millis_to_secs(ban_time_ms)),
    ])
    .map_err(Into::into)
}

pub fn get_ban_records(conn: &Connection, now_ms: u64) -> DBResult<Vec<(Vec<u8>, u64)>> {
    let mut stmt =
        conn.prepare("SELECT ip, ban_time_secs FROM ban_list WHERE ban_time_secs > :now")?;
    let rows = stmt.query_map_named(&[(":now", &millis_to_secs(now_ms))], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, secs_to_millis(row.get(1)?)))
    })?;
    Result::from_iter(rows).map_err(Into::into)
}

pub fn clear_expires_banned_ip(conn: &Connection, now_ms: u64) -> DBResult<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT ip FROM ban_list WHERE ban_time_secs < :now")?;
    let rows = stmt.query_map_named(&[(":now", &millis_to_secs(now_ms))], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;
    let mut stmt = conn.prepare("DELETE FROM ban_list WHERE ban_time_secs < :now")?;
    stmt.execute_named(&[(":now", &millis_to_secs(now_ms))])?;
    Result::from_iter(rows).map_err(Into::into)
}

fn status_to_u8(status: Status) -> u8 {
    status as u8
}

fn u8_to_status(i: u8) -> Status {
    Status::from(i)
}

fn secs_to_millis(secs: u32) -> u64 {
    u64::from(secs) * 1000
}

fn millis_to_secs(millis: u64) -> u32 {
    (millis / 1000) as u32
}

fn endpoint_to_bool(endpoint: SessionType) -> bool {
    endpoint.is_inbound()
}

fn bool_to_endpoint(is_inbound: bool) -> SessionType {
    if is_inbound {
        SessionType::Inbound
    } else {
        SessionType::Outbound
    }
}

fn network_group_to_bytes(network_group: &Group) -> Vec<u8> {
    format!("{:?}", network_group).into_bytes()
}
