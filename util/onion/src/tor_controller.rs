use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{debug, error, info, warn};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

use crate::{TorEventHandlerFn, TorSecretKeyV3};

const COOKIE_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 32;
const TOR_REPLY_OK: u16 = 250;
const MAX_LINE_LENGTH: usize = 100_000;
const TOR_SAFE_SERVER_KEY: &[u8] = b"Tor safe cookie authentication server-to-controller hash";
const TOR_SAFE_CLIENT_KEY: &[u8] = b"Tor safe cookie authentication controller-to-server hash";

#[derive(Debug)]
pub enum ConnError {
    Io(std::io::Error),
    InvalidResponseCode(u16),
    InvalidResponse(String),
}

impl Display for ConnError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "IO error: {}", err),
            Self::InvalidResponseCode(code) => write!(f, "invalid Tor response code: {}", code),
            Self::InvalidResponse(response) => write!(f, "invalid Tor response: {}", response),
        }
    }
}

impl std::error::Error for ConnError {}

impl From<std::io::Error> for ConnError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Debug)]
struct TorControlReply {
    code: u16,
    lines: Vec<String>,
}

#[derive(Debug)]
struct ProtocolInfo {
    auth_methods: HashSet<String>,
    cookie_file: Option<PathBuf>,
}

struct TorControlConnection {
    reader: BufReader<ReadHalf<TcpStream>>,
    writer: WriteHalf<TcpStream>,
}

impl TorControlConnection {
    fn new(stream: TcpStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn command(&mut self, command: &str) -> Result<TorControlReply, ConnError> {
        self.writer.write_all(command.as_bytes()).await?;
        self.writer.write_all(b"\r\n").await?;
        self.writer.flush().await?;
        self.read_reply().await
    }

    async fn read_reply(&mut self) -> Result<TorControlReply, ConnError> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let bytes = self.reader.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(ConnError::InvalidResponse(
                    "Tor control connection closed".to_string(),
                ));
            }
            if line.len() > MAX_LINE_LENGTH {
                return Err(ConnError::InvalidResponse(format!(
                    "Tor control reply line exceeds {} bytes",
                    MAX_LINE_LENGTH
                )));
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.len() < 4 {
                return Err(ConnError::InvalidResponse(line.to_string()));
            }
            let code = line[..3]
                .parse::<u16>()
                .map_err(|_| ConnError::InvalidResponse(line.to_string()))?;
            let separator = line.as_bytes()[3] as char;

            if code >= 600 {
                if separator == ' ' {
                    lines.clear();
                }
                continue;
            }

            lines.push(line[4..].to_string());
            if separator == ' ' {
                return Ok(TorControlReply { code, lines });
            }
        }
    }
}

/// A controller for a Tor server.
pub struct TorController {
    inner: TorControlConnection,
}

impl TorController {
    /// Create a new TorController instance.
    pub async fn new(
        tor_controller_url: String,
        tor_password: Option<String>,
        _event_handler: Option<TorEventHandlerFn>,
    ) -> Result<Self, Error> {
        let stream = TcpStream::connect(tor_controller_url.clone())
            .await
            .map_err(|err| {
                InternalErrorKind::Other.other(format!(
                    "Failed to connect to tor controller {}: {:?}",
                    tor_controller_url, err
                ))
            })?;

        let mut conn = TorControlConnection::new(stream);
        authenticate(tor_password, &mut conn).await?;
        Ok(TorController { inner: conn })
    }

    /// get tor server's status
    pub async fn get_bootstrap_phase(&mut self) -> Result<String, ConnError> {
        self.get_info_unquote("status/bootstrap-phase").await
    }

    /// get tor server's version
    pub async fn get_version(&mut self) -> Result<String, ConnError> {
        self.get_info("version").await
    }

    /// get tor server's uptime
    pub async fn get_uptime(&mut self) -> Result<Duration, ConnError> {
        let uptime = self.get_info("uptime").await.map_err(|err| {
            warn!(
                "failed to get uptime; the Tor controller may not expose 'uptime' (older Tor versions) or returned an error: {}",
                err
            );
            err
        })?;
        debug!("tor server's uptime is {} seconds", uptime);
        let secs: u64 = uptime.parse().map_err(|err| {
            ConnError::InvalidResponse(format!("failed to parse uptime {} to u64 {}", uptime, err))
        })?;
        Ok(Duration::from_secs(secs))
    }

    /// Waits asynchronously until the Tor server has completed its bootstrap process.
    pub async fn wait_tor_server_bootstrap_done(&mut self) -> Result<(), Error> {
        info!("waiting tor server bootstrap");
        loop {
            if ckb_stop_handler::has_received_stop_signal() {
                return Err(InternalErrorKind::Other
                    .other("Received stop signal")
                    .into());
            }
            let bootstrap_done = match self.get_bootstrap_phase().await {
                Ok(info) => {
                    info!("Waiting Tor bootstrapping: current status: {:?}", info);
                    info.contains("Done")
                }
                Err(err) => {
                    error!("Failed to get tor bootstrap status: {:?}", err);
                    return Err(InternalErrorKind::Other
                        .other(format!("Failed to get tor bootstrap status: {:?}", err))
                        .into());
                }
            };
            if bootstrap_done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        info!("Tor server bootstrap done!");
        Ok(())
    }

    /// Add a new v3 onion service to the Tor server.
    pub async fn add_onion_v3(
        &mut self,
        key: TorSecretKeyV3,
        listeners: &mut impl Iterator<Item = &(u16, SocketAddr)>,
    ) -> Result<(), ConnError> {
        let mut command = format!(
            "ADD_ONION ED25519-V3:{} Flags=DiscardPK",
            key.to_tor_key_blob()
        );
        let mut ports = HashSet::new();
        let mut has_listener = false;
        for (port, address) in listeners {
            if !ports.insert(*port) {
                return Err(ConnError::InvalidResponse(format!(
                    "duplicate onion listener port {}",
                    port
                )));
            }
            has_listener = true;
            command.push_str(&format!(" Port={},{}", port, address));
        }
        if !has_listener {
            return Err(ConnError::InvalidResponse(
                "missing onion listener".to_string(),
            ));
        }

        let reply = self.inner.command(&command).await?;
        expect_ok(reply).map(|_| ())
    }

    async fn get_info(&mut self, key: &str) -> Result<String, ConnError> {
        let reply = expect_ok(self.inner.command(&format!("GETINFO {}", key)).await?)?;
        let prefix = format!("{}=", key);
        reply
            .lines
            .into_iter()
            .find_map(|line| line.strip_prefix(&prefix).map(ToOwned::to_owned))
            .ok_or_else(|| ConnError::InvalidResponse(format!("GETINFO {} missing value", key)))
    }

    async fn get_info_unquote(&mut self, key: &str) -> Result<String, ConnError> {
        let value = self.get_info(key).await?;
        Ok(unquote_tor_string(&value).unwrap_or(value))
    }
}

async fn authenticate(
    tor_password: Option<String>,
    conn: &mut TorControlConnection,
) -> Result<(), Error> {
    let proto_info = load_protocol_info(conn).await.map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to load protocol info: {:?}", err))
    })?;
    for method in &proto_info.auth_methods {
        info!("Tor Server Controller supports auth method: {:?}", method);
    }

    if proto_info.auth_methods.contains("NULL") {
        let reply = conn.command("AUTHENTICATE").await.map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to authenticate with null: {:?}", err))
        })?;
        expect_ok(reply).map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to authenticate with null: {:?}", err))
        })?;
        if tor_password.is_some() {
            warn!("Password not required for the Tor controller, but `tor_password` is configured in [network.onion].");
        }
        return Ok(());
    }

    if proto_info.auth_methods.contains("HASHEDPASSWORD") {
        match tor_password {
            Some(tor_password) => {
                let command = format!("AUTHENTICATE {}", quote_tor_string(&tor_password));
                let reply = conn.command(&command).await.map_err(|err| {
                    InternalErrorKind::Other
                        .other(format!("Failed to authenticate with password: {:?}", err))
                })?;
                expect_ok(reply).map_err(|err| {
                    InternalErrorKind::Other
                        .other(format!("Failed to authenticate with password: {:?}", err))
                })?;
                return Ok(());
            }
            None => warn!("Tor server requires a password, but none is configured"),
        }
    }

    if proto_info.auth_methods.contains("COOKIE") || proto_info.auth_methods.contains("SAFECOOKIE")
    {
        let cookie = load_auth_cookie(&proto_info).await?;
        if proto_info.auth_methods.contains("COOKIE") {
            debug!("Using Cookie auth method...");
            let reply = conn
                .command(&format!("AUTHENTICATE {}", hex::encode_upper(cookie)))
                .await
                .map_err(|err| {
                    InternalErrorKind::Other
                        .other(format!("Failed to authenticate with cookie: {:?}", err))
                })?;
            expect_ok(reply).map_err(|err| {
                InternalErrorKind::Other
                    .other(format!("Failed to authenticate with cookie: {:?}", err))
            })?;
        } else {
            debug!("Using SafeCookie auth method...");
            authenticate_safecookie(conn, &cookie)
                .await
                .map_err(|err| {
                    InternalErrorKind::Other.other(format!(
                        "Failed to authenticate with safe cookie: {:?}",
                        err
                    ))
                })?;
        }
        return Ok(());
    }

    Err(InternalErrorKind::Other
        .other(format!(
            "Tor server does not support any authentication method; proto_info: {:?}",
            proto_info
        ))
        .into())
}

async fn load_protocol_info(conn: &mut TorControlConnection) -> Result<ProtocolInfo, ConnError> {
    let reply = expect_ok(conn.command("PROTOCOLINFO 1").await?)?;
    let mut auth_methods = HashSet::new();
    let mut cookie_file = None;

    for line in reply.lines {
        let (kind, rest) = split_tor_reply_line(&line);
        if kind != "AUTH" {
            continue;
        }
        let mapping = parse_tor_reply_mapping(rest)?;
        if let Some(methods) = mapping.get("METHODS") {
            auth_methods.extend(methods.split(',').map(ToOwned::to_owned));
        }
        if let Some(path) = mapping.get("COOKIEFILE") {
            cookie_file = Some(PathBuf::from(path));
        }
    }

    Ok(ProtocolInfo {
        auth_methods,
        cookie_file,
    })
}

async fn authenticate_safecookie(
    conn: &mut TorControlConnection,
    cookie: &[u8],
) -> Result<(), ConnError> {
    let mut client_nonce = [0u8; NONCE_LENGTH];
    rand::thread_rng().fill_bytes(&mut client_nonce);
    let reply = expect_ok(
        conn.command(&format!(
            "AUTHCHALLENGE SAFECOOKIE {}",
            hex::encode_upper(client_nonce)
        ))
        .await?,
    )?;
    let line = reply
        .lines
        .first()
        .ok_or_else(|| ConnError::InvalidResponse("missing AUTHCHALLENGE reply".to_string()))?;
    let (kind, rest) = split_tor_reply_line(line);
    if kind != "AUTHCHALLENGE" {
        return Err(ConnError::InvalidResponse(line.clone()));
    }
    let mapping = parse_tor_reply_mapping(rest)?;
    let server_hash = decode_hex_mapping(&mapping, "SERVERHASH")?;
    let server_nonce = decode_hex_mapping(&mapping, "SERVERNONCE")?;
    if server_nonce.len() != NONCE_LENGTH {
        return Err(ConnError::InvalidResponse(format!(
            "invalid SAFECOOKIE server nonce length {}",
            server_nonce.len()
        )));
    }

    let expected_server_hash =
        compute_safecookie_hmac(TOR_SAFE_SERVER_KEY, cookie, &client_nonce, &server_nonce)?;
    if server_hash != expected_server_hash {
        return Err(ConnError::InvalidResponse(
            "SAFECOOKIE server hash mismatch".to_string(),
        ));
    }

    let client_hash =
        compute_safecookie_hmac(TOR_SAFE_CLIENT_KEY, cookie, &client_nonce, &server_nonce)?;
    let reply = conn
        .command(&format!("AUTHENTICATE {}", hex::encode_upper(client_hash)))
        .await?;
    expect_ok(reply).map(|_| ())
}

fn compute_safecookie_hmac(
    key: &[u8],
    cookie: &[u8],
    client_nonce: &[u8],
    server_nonce: &[u8],
) -> Result<Vec<u8>, ConnError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|err| ConnError::InvalidResponse(err.to_string()))?;
    mac.update(cookie);
    mac.update(client_nonce);
    mac.update(server_nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

async fn load_auth_cookie(proto_info: &ProtocolInfo) -> Result<Vec<u8>, Error> {
    let cookie_path = proto_info.cookie_file.as_ref().ok_or_else(|| {
        InternalErrorKind::Other.other("Tor server did not provide cookie file path")
    })?;
    let mut file = File::open(cookie_path).await.map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to open cookie file: {:?}", err))
    })?;
    let mut cookie = Vec::new();
    file.read_to_end(&mut cookie).await.map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to read cookie file: {:?}", err))
    })?;
    if cookie.len() != COOKIE_LENGTH {
        return Err(InternalErrorKind::Other
            .other(format!(
                "Invalid cookie length: expected {}, got {}",
                COOKIE_LENGTH,
                cookie.len()
            ))
            .into());
    }
    Ok(cookie)
}

fn expect_ok(reply: TorControlReply) -> Result<TorControlReply, ConnError> {
    if reply.code == TOR_REPLY_OK {
        Ok(reply)
    } else {
        Err(ConnError::InvalidResponseCode(reply.code))
    }
}

fn split_tor_reply_line(line: &str) -> (&str, &str) {
    match line.split_once(' ') {
        Some((kind, rest)) => (kind, rest),
        None => (line, ""),
    }
}

fn parse_tor_reply_mapping(input: &str) -> Result<HashMap<String, String>, ConnError> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut mapping = HashMap::new();

    while index < bytes.len() {
        let key_start = index;
        while index < bytes.len() && bytes[index] != b'=' && bytes[index] != b' ' {
            index += 1;
        }
        if index == bytes.len() || bytes[index] == b' ' {
            break;
        }
        let key = &input[key_start..index];
        index += 1;

        let value = if index < bytes.len() && bytes[index] == b'"' {
            index += 1;
            let mut raw = String::new();
            let mut escaped = false;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if escaped {
                    raw.push('\\');
                    raw.push(byte as char);
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    break;
                } else {
                    raw.push(byte as char);
                }
            }
            unescape_tor_string(&raw)?
        } else {
            let value_start = index;
            while index < bytes.len() && bytes[index] != b' ' {
                index += 1;
            }
            input[value_start..index].to_string()
        };

        if index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        mapping.insert(key.to_string(), value);
    }

    Ok(mapping)
}

fn unquote_tor_string(value: &str) -> Option<String> {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .and_then(|value| unescape_tor_string(value).ok())
}

fn unescape_tor_string(value: &str) -> Result<String, ConnError> {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(ConnError::InvalidResponse(
                "unterminated escape".to_string(),
            ));
        };
        match escaped {
            'n' => output.push('\n'),
            't' => output.push('\t'),
            'r' => output.push('\r'),
            '0'..='7' => {
                let mut octal = escaped.to_digit(8).unwrap();
                for _ in 0..2 {
                    match chars.peek().and_then(|ch| ch.to_digit(8)) {
                        Some(digit) if octal < 32 => {
                            octal = octal * 8 + digit;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                output.push(char::from(octal as u8));
            }
            other => output.push(other),
        }
    }
    Ok(output)
}

fn quote_tor_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn decode_hex_mapping(mapping: &HashMap<String, String>, key: &str) -> Result<Vec<u8>, ConnError> {
    let value = mapping
        .get(key)
        .ok_or_else(|| ConnError::InvalidResponse(format!("missing {}", key)))?;
    hex::decode(value).map_err(|err| ConnError::InvalidResponse(err.to_string()))
}
