use std::borrow::Cow;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;

use ckb_async_runtime::Handle;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Error type for Tor control protocol operations.
#[derive(Debug, thiserror::Error)]
pub enum ConnError {
    /// IO error from the underlying stream.
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    /// Hex encoding/decoding error.
    #[error("hex error: {0}")]
    Hex(#[from] hex::FromHexError),

    /// Invalid response code from Tor controller.
    #[error("invalid response code: {0}")]
    InvalidResponseCode(u16),

    /// Invalid response format.
    #[error("invalid response format")]
    InvalidFormat,

    /// Protocol info was already fetched.
    #[error("protocol info already fetched")]
    InfoFetchedTwice,

    /// Authentication failed.
    #[error("authentication failed")]
    AuthFailed,

    /// No supported authentication method.
    #[error("no supported authentication method")]
    UnsupportedAuthMethod,

    /// HMAC initialization failed.
    #[error("HMAC initialization failed")]
    HmacInit,
}

/// Tor authentication method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TorAuthMethod {
    /// No authentication.
    Null,
    /// Hashed password authentication.
    HashedPassword,
    /// Cookie authentication.
    Cookie,
    /// Safe cookie authentication.
    SafeCookie,
}

impl FromStr for TorAuthMethod {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "NULL" => Ok(TorAuthMethod::Null),
            "HASHEDPASSWORD" => Ok(TorAuthMethod::HashedPassword),
            "COOKIE" => Ok(TorAuthMethod::Cookie),
            "SAFECOOKIE" => Ok(TorAuthMethod::SafeCookie),
            _ => Err(()),
        }
    }
}

/// Tor authentication data.
#[derive(Debug, Clone)]
pub enum TorAuthData<'a> {
    /// No authentication.
    Null,
    /// Hashed password.
    HashedPassword(Cow<'a, str>),
    /// Cookie content.
    Cookie(Cow<'a, [u8]>),
    /// Safe cookie content.
    SafeCookie(Cow<'a, [u8]>),
}

#[allow(dead_code)]
/// Protocol information returned by `PROTOCOLINFO`.
#[derive(Debug, Clone)]
pub struct ProtocolInfo {
    /// Tor version string.
    pub version: String,
    /// Supported authentication methods.
    pub auth_methods: HashSet<TorAuthMethod>,
    /// Cookie file path, if any.
    pub cookie_file: Option<String>,
}

struct Response {
    code: u16,
    lines: Vec<String>,
}

pub struct TorConnection {
    write: OwnedWriteHalf,
    line_rx: mpsc::UnboundedReceiver<String>,
}

impl TorConnection {
    /// Connect to a Tor control port using the given TCP stream halves.
    /// A background task is spawned to read lines from the stream; when it
    /// terminates, a value is sent on an internal disconnect channel.
    pub(crate) fn connect(stream: TcpStream, handle: Handle) -> Self {
        let (read, write) = stream.into_split();
        let (line_tx, line_rx) = mpsc::unbounded_channel();

        handle.spawn(async move {
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            loop {
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.ends_with("\r\n") {
                            line.truncate(line.len() - 2);
                        } else if line.ends_with('\n') {
                            line.pop();
                        }
                        let taken_line = std::mem::take(&mut line);
                        if line_tx.send(taken_line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            drop(line_tx);
        });

        TorConnection { write, line_rx }
    }

    /// Load protocol information from the Tor controller.
    pub(crate) async fn load_protocol_info(mut self) -> Result<(Self, ProtocolInfo), ConnError> {
        self.send_command("PROTOCOLINFO 1\r\n").await?;
        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }
        let info = parse_protocol_info(&resp.lines)?;
        Ok((self, info))
    }

    /// Authenticate with the Tor controller.
    pub(crate) async fn authenticate(mut self, data: &TorAuthData<'_>) -> Result<Self, ConnError> {
        match data {
            TorAuthData::Null => {
                self.send_command("AUTHENTICATE\r\n").await?;
            }
            TorAuthData::HashedPassword(password) => {
                self.send_command(&format!(
                    "AUTHENTICATE {}\r\n",
                    quote_string(password.as_bytes())
                ))
                .await?;
            }
            TorAuthData::Cookie(cookie) => {
                self.send_command(&format!(
                    "AUTHENTICATE {}\r\n",
                    hex::encode_upper(cookie.as_ref())
                ))
                .await?;
            }
            TorAuthData::SafeCookie(cookie) => {
                return self.authenticate_safe_cookie(cookie.as_ref()).await;
            }
        }

        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }
        Ok(self)
    }

    /// Get a single INFO value from the Tor controller.
    pub async fn get_info(&mut self, key: &str) -> Result<String, ConnError> {
        self.send_command(&format!("GETINFO {}\r\n", key)).await?;
        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }
        if resp.lines.is_empty() || resp.lines.last() != Some(&"OK".to_string()) {
            return Err(ConnError::InvalidFormat);
        }

        let key_prefix = format!("{}=", key);
        let value_line = resp
            .lines
            .iter()
            .find(|l| l.starts_with(&key_prefix))
            .ok_or(ConnError::InvalidFormat)?;
        Ok(value_line[key_prefix.len()..].to_string())
    }

    /// Get a single INFO value and unquote it if it is a quoted string.
    pub async fn get_info_unquote(&mut self, key: &str) -> Result<String, ConnError> {
        let value = self.get_info(key).await?;
        match unquote_string(&value) {
            Ok(unquoted) => Ok(unquoted),
            Err(_) => Ok(value),
        }
    }

    /// Add an onion v3 service.
    pub async fn add_onion_v3(
        &mut self,
        key: &crate::onion::TorSecretKeyV3,
        listeners: &[(u16, SocketAddr)],
    ) -> Result<(), ConnError> {
        let mut cmd = format!(
            "ADD_ONION ED25519-V3:{} Flags=DiscardPK",
            key.as_tor_proto_encoded()
        );
        for (port, addr) in listeners {
            cmd.push_str(&format!(" Port={},{}", port, addr));
        }
        cmd.push_str("\r\n");

        self.send_command(&cmd).await?;
        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }
        Ok(())
    }

    async fn send_command(&mut self, cmd: &str) -> Result<(), ConnError> {
        self.write.write_all(cmd.as_bytes()).await?;
        self.write.flush().await?;
        Ok(())
    }

    async fn receive_response(&mut self) -> Result<Response, ConnError> {
        let mut lines = Vec::new();
        let mut code = None;

        loop {
            let line = self.line_rx.recv().await.ok_or_else(|| {
                ConnError::IOError(std::io::Error::other("Tor control connection closed"))
            })?;

            if line.len() < 4 {
                return Err(ConnError::InvalidFormat);
            }

            let parsed_code = line[..3]
                .parse::<u16>()
                .map_err(|_| ConnError::InvalidFormat)?;
            let sep = line.as_bytes()[3];
            let content = line[4..].to_string();

            if let Some(c) = code {
                if c != parsed_code {
                    return Err(ConnError::InvalidFormat);
                }
            } else {
                code = Some(parsed_code);
            }

            match sep {
                b' ' => {
                    lines.push(content);
                    break;
                }
                b'-' => {
                    lines.push(content);
                }
                b'+' => {
                    let mut data = content;
                    loop {
                        let data_line = self.line_rx.recv().await.ok_or_else(|| {
                            ConnError::IOError(std::io::Error::other(
                                "Tor control connection closed",
                            ))
                        })?;
                        if data_line == "." {
                            break;
                        }
                        data.push('\n');
                        data.push_str(&data_line);
                    }
                    lines.push(data);
                }
                _ => return Err(ConnError::InvalidFormat),
            }
        }

        Ok(Response {
            code: code.unwrap(),
            lines,
        })
    }

    async fn authenticate_safe_cookie(mut self, cookie: &[u8]) -> Result<Self, ConnError> {
        let mut client_nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut client_nonce);

        self.send_command(&format!(
            "AUTHCHALLENGE SAFECOOKIE {}\r\n",
            hex::encode_upper(client_nonce)
        ))
        .await?;
        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }

        let (server_hash, server_nonce) = parse_auth_challenge_response(&resp.lines)?;

        let server_hash_expected = hmac_sha256(
            cookie,
            &[
                b"Tor safe cookie authentication server-to-controller hash",
                &client_nonce,
                &server_nonce,
            ],
        )?;
        if server_hash_expected != server_hash {
            return Err(ConnError::AuthFailed);
        }

        let client_hash = hmac_sha256(
            cookie,
            &[
                b"Tor safe cookie authentication controller-to-server hash",
                &client_nonce,
                &server_nonce,
            ],
        )?;

        self.send_command(&format!(
            "AUTHENTICATE {}\r\n",
            hex::encode_upper(client_hash)
        ))
        .await?;
        let resp = self.receive_response().await?;
        if resp.code != 250 {
            return Err(ConnError::InvalidResponseCode(resp.code));
        }
        Ok(self)
    }
}

fn parse_protocol_info(lines: &[String]) -> Result<ProtocolInfo, ConnError> {
    if lines.is_empty() || lines[0] != "PROTOCOLINFO 1" {
        return Err(ConnError::InvalidFormat);
    }
    if lines.last().map(|s| s.as_str()) != Some("OK") {
        return Err(ConnError::InvalidFormat);
    }

    let mut auth_methods = HashSet::new();
    let mut cookie_file = None;
    let mut version = None;

    for line in &lines[1..lines.len() - 1] {
        if let Some(rest) = line.strip_prefix("AUTH METHODS=") {
            let (methods_str, cookie_str) = match rest.find(' ') {
                Some(idx) => (&rest[..idx], rest[idx..].trim()),
                None => (rest, ""),
            };

            for m in methods_str.split(',') {
                if let Ok(method) = TorAuthMethod::from_str(m) {
                    auth_methods.insert(method);
                }
            }

            if let Some(cookie_part) = cookie_str.strip_prefix("COOKIEFILE=") {
                cookie_file = Some(unquote_string(cookie_part)?);
            }
        } else if let Some(rest) = line.strip_prefix("VERSION Tor=") {
            version = Some(unquote_string(rest)?);
        }
    }

    if auth_methods.is_empty() {
        return Err(ConnError::InvalidFormat);
    }

    Ok(ProtocolInfo {
        version: version.ok_or(ConnError::InvalidFormat)?,
        auth_methods,
        cookie_file,
    })
}

fn parse_auth_challenge_response(lines: &[String]) -> Result<([u8; 32], [u8; 32]), ConnError> {
    if lines.len() != 1 {
        return Err(ConnError::InvalidFormat);
    }
    let line = &lines[0];
    let prefix = "AUTHCHALLENGE ";
    if !line.starts_with(prefix) {
        return Err(ConnError::InvalidFormat);
    }
    let rest = &line[prefix.len()..];
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() != 2 {
        return Err(ConnError::InvalidFormat);
    }

    let server_hash_hex = parts[0]
        .strip_prefix("SERVERHASH=")
        .ok_or(ConnError::InvalidFormat)?;
    let server_nonce_hex = parts[1]
        .strip_prefix("SERVERNONCE=")
        .ok_or(ConnError::InvalidFormat)?;

    let server_hash = hex_to_array32(server_hash_hex)?;
    let server_nonce = hex_to_array32(server_nonce_hex)?;

    Ok((server_hash, server_nonce))
}

fn hex_to_array32(s: &str) -> Result<[u8; 32], ConnError> {
    let bytes = hex::decode(s)?;
    if bytes.len() != 32 {
        return Err(ConnError::InvalidFormat);
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], ConnError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| ConnError::HmacInit)?;
    for part in parts {
        mac.update(part);
    }
    let result = mac.finalize();
    Ok(result.into_bytes().into())
}

fn quote_string(s: &[u8]) -> String {
    let mut result = String::with_capacity(s.len() + 2);
    result.push('"');
    for &b in s {
        match b {
            b'\\' => result.push_str("\\\\"),
            b'"' => result.push_str("\\\""),
            b'\r' => result.push_str("\\r"),
            b'\n' => result.push_str("\\n"),
            b'\t' => result.push_str("\\t"),
            0x00..=0x1F => result.push_str(&format!("\\x{:02x}", b)),
            _ => result.push(b as char),
        }
    }
    result.push('"');
    result
}

fn unquote_string(s: &str) -> Result<String, ConnError> {
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return Err(ConnError::InvalidFormat);
    }
    let mut result = String::new();
    let mut chars = s[1..s.len() - 1].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('r') => result.push('\r'),
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('x') => {
                    let a = chars.next().ok_or(ConnError::InvalidFormat)?;
                    let b = chars.next().ok_or(ConnError::InvalidFormat)?;
                    let hex_str = format!("{}{}", a, b);
                    let byte =
                        u8::from_str_radix(&hex_str, 16).map_err(|_| ConnError::InvalidFormat)?;
                    result.push(byte as char);
                }
                _ => return Err(ConnError::InvalidFormat),
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_unquote() {
        let s = b"hello world";
        let quoted = quote_string(s);
        assert_eq!(quoted, "\"hello world\"");
        assert_eq!(unquote_string(&quoted).unwrap(), "hello world");

        let s = b"a\"b\\c\n";
        let quoted = quote_string(s);
        assert_eq!(unquote_string(&quoted).unwrap().as_bytes(), s);
    }

    #[test]
    fn test_parse_protocol_info() {
        let lines = vec![
            "PROTOCOLINFO 1".to_string(),
            "AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE=\"/path/to/cookie\"".to_string(),
            "VERSION Tor=\"0.4.5.6\"".to_string(),
            "OK".to_string(),
        ];
        let info = parse_protocol_info(&lines).unwrap();
        assert!(info.auth_methods.contains(&TorAuthMethod::Cookie));
        assert!(info.auth_methods.contains(&TorAuthMethod::SafeCookie));
        assert_eq!(info.cookie_file, Some("/path/to/cookie".to_string()));
        assert_eq!(info.version, "0.4.5.6");
    }

    #[test]
    fn test_protocol_info_missing_ok() {
        let lines = vec![
            "PROTOCOLINFO 1".to_string(),
            "AUTH METHODS=NULL".to_string(),
            "VERSION Tor=\"0.4.5.6\"".to_string(),
        ];
        assert!(parse_protocol_info(&lines).is_err());
    }

    #[test]
    fn test_protocol_info_unknown_method() {
        let lines = vec![
            "PROTOCOLINFO 1".to_string(),
            "AUTH METHODS=UNKNOWN,COOKIE".to_string(),
            "VERSION Tor=\"0.4.5.6\"".to_string(),
            "OK".to_string(),
        ];
        let info = parse_protocol_info(&lines).unwrap();
        assert!(!info.auth_methods.contains(&TorAuthMethod::Null));
        assert!(info.auth_methods.contains(&TorAuthMethod::Cookie));
    }

    #[test]
    fn test_parse_auth_challenge_response() {
        let hash = "a".repeat(64);
        let nonce = "b".repeat(64);
        let lines = vec![format!(
            "AUTHCHALLENGE SERVERHASH={} SERVERNONCE={}",
            hash, nonce
        )];
        let (h, n) = parse_auth_challenge_response(&lines).unwrap();
        assert_eq!(h.as_slice(), &[0xaa; 32]);
        assert_eq!(n.as_slice(), &[0xbb; 32]);
    }

    #[test]
    fn test_auth_challenge_bad_format() {
        let lines = vec!["AUTHCHALLENGE BAD".to_string()];
        assert!(parse_auth_challenge_response(&lines).is_err());

        let lines = vec!["AUTHCHALLENGE SERVERHASH=abcd".to_string()];
        assert!(parse_auth_challenge_response(&lines).is_err());
    }
}
