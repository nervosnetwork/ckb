use std::borrow::Cow;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;

use ckb_async_runtime::Handle;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use strum::FromRepr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Tor control protocol status codes.
///
/// See <https://spec.torproject.org/control-spec/replies.html> for the full table.
#[derive(FromRepr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StatusCode {
    /// 250 OK
    OK = 250,
    /// 451 Resource temporarily unavailable.
    Unavailable = 451,
    /// 500 Command syntax error.
    SyntaxError = 500,
    /// 510 Command not implemented.
    NotImplemented = 510,
    /// 512 Argument too long.
    ArgTooLong = 512,
    /// 513 Internal error.
    InternalError = 513,
    /// 514 Unrecognized command.
    UnrecognizedCommand = 514,
    /// 515 Authentication required.
    AuthRequired = 515,
    /// 550 Permission denied.
    PermissionDenied = 550,
    /// 551 Client sent invalid authentication.
    AuthFailed = 551,
    /// 552 Unrecognized key.
    UnrecognizedKey = 552,
    /// 553 Cookie value incorrect.
    CookieIncorrect = 553,
    /// 650 Events disabled.
    EventsDisabled = 650,
    /// 651 Events enabled.
    EventsEnabled = 651,
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusCode::OK => write!(f, "250 OK"),
            StatusCode::Unavailable => write!(f, "451 Unavailable"),
            StatusCode::SyntaxError => write!(f, "500 Syntax error"),
            StatusCode::NotImplemented => write!(f, "510 Not implemented"),
            StatusCode::ArgTooLong => write!(f, "512 Argument too long"),
            StatusCode::InternalError => write!(f, "513 Internal error"),
            StatusCode::UnrecognizedCommand => write!(f, "514 Unrecognized command"),
            StatusCode::AuthRequired => write!(f, "515 Authentication required"),
            StatusCode::PermissionDenied => write!(f, "550 Permission denied"),
            StatusCode::AuthFailed => write!(f, "551 Authentication failed"),
            StatusCode::UnrecognizedKey => write!(f, "552 Unrecognized key"),
            StatusCode::CookieIncorrect => write!(f, "553 Cookie incorrect"),
            StatusCode::EventsDisabled => write!(f, "650 Events disabled"),
            StatusCode::EventsEnabled => write!(f, "651 Events enabled"),
        }
    }
}

/// A Tor control protocol command, terminated with `\r\n` on the wire.
struct Command {
    buf: Vec<u8>,
}

impl Command {
    fn new(name: &str) -> Self {
        let mut buf = Vec::with_capacity(name.len());
        buf.extend_from_slice(name.as_bytes());
        Self { buf }
    }

    fn arg(mut self, s: &str) -> Self {
        self.buf.push(b' ');
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    fn arg_bytes(mut self, s: &[u8]) -> Self {
        self.buf.push(b' ');
        self.buf.extend_from_slice(s);
        self
    }
}

impl AsRef<[u8]> for Command {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

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
    InvalidResponseCode(StatusCode),

    /// Invalid response format.
    #[error("invalid response format: {0}")]
    InvalidFormat(String),

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
    code: StatusCode,
    lines: Vec<String>,
}

impl Response {
    fn expect_ok(&self) -> Result<(), ConnError> {
        if self.code != StatusCode::OK {
            return Err(ConnError::InvalidResponseCode(self.code));
        }
        Ok(())
    }
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
        self.send_command(Command::new("PROTOCOLINFO").arg("1"))
            .await?;
        let resp = self.receive_response().await?;
        resp.expect_ok()?;
        let info = parse_protocol_info(&resp.lines)?;
        Ok((self, info))
    }

    /// Authenticate with the Tor controller.
    pub(crate) async fn authenticate(mut self, data: &TorAuthData<'_>) -> Result<Self, ConnError> {
        match data {
            TorAuthData::Null => {
                self.send_command(Command::new("AUTHENTICATE")).await?;
            }
            TorAuthData::HashedPassword(password) => {
                self.send_command(
                    Command::new("AUTHENTICATE").arg_bytes(&quote_string(password.as_bytes())),
                )
                .await?;
            }
            TorAuthData::Cookie(cookie) => {
                self.send_command(
                    Command::new("AUTHENTICATE").arg(&hex::encode_upper(cookie.as_ref())),
                )
                .await?;
            }
            TorAuthData::SafeCookie(cookie) => {
                return self.authenticate_safe_cookie(cookie.as_ref()).await;
            }
        }

        let resp = self.receive_response().await?;
        resp.expect_ok()?;
        Ok(self)
    }

    /// Get a single INFO value from the Tor controller.
    pub async fn get_info(&mut self, key: &str) -> Result<String, ConnError> {
        self.send_command(Command::new("GETINFO").arg(key)).await?;
        let resp = self.receive_response().await?;
        resp.expect_ok()?;
        if resp.lines.is_empty() || resp.lines.last() != Some(&"OK".to_string()) {
            return Err(ConnError::InvalidFormat(
                "GETINFO response missing trailing OK".into(),
            ));
        }

        let key_prefix = format!("{}=", key);
        let value_line = resp
            .lines
            .iter()
            .find(|l| l.starts_with(&key_prefix))
            .ok_or_else(|| ConnError::InvalidFormat(format!("GETINFO key '{}' not found", key)))?;
        Ok(value_line[key_prefix.len()..].to_string())
    }

    /// Get a single INFO value and unquote it if it is a quoted string.
    pub async fn get_info_unquote(&mut self, key: &str) -> Result<String, ConnError> {
        let value = self.get_info(key).await?;
        match unquote_string_to_string(&value) {
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
        let mut cmd = Command::new("ADD_ONION")
            .arg(&format!("ED25519-V3:{}", key.as_tor_proto_encoded()))
            .arg("Flags=DiscardPK");
        for (port, addr) in listeners {
            cmd = cmd.arg(&format!("Port={},{}", port, addr));
        }

        self.send_command(cmd).await?;
        let resp = self.receive_response().await?;
        resp.expect_ok()?;
        Ok(())
    }

    async fn send_command(&mut self, cmd: impl AsRef<[u8]>) -> Result<(), ConnError> {
        self.write.write_all(cmd.as_ref()).await?;
        self.write.write_all(b"\r\n").await?;
        self.write.flush().await?;
        Ok(())
    }

    /// Parses a Tor control protocol response from the channel.
    ///
    /// Each response line has format `XXx Sep Content` where `Sep` is:
    /// - `' '` — last line of the response (loop terminates)
    /// - `'-'` — mid-line in a multi-line reply
    /// - `'+'` — start of a data block, terminated by a line containing only `.`
    ///
    /// Per the spec, every line in a response must carry the same status code.
    async fn receive_response(&mut self) -> Result<Response, ConnError> {
        let mut lines = Vec::new();
        let mut first_code: Option<StatusCode> = None;

        loop {
            let line = self.line_rx.recv().await.ok_or_else(|| {
                ConnError::IOError(std::io::Error::other("Tor control connection closed"))
            })?;

            if line.len() < 4 {
                return Err(ConnError::InvalidFormat(format!(
                    "response line too short: {:?}",
                    line
                )));
            }

            let parsed_code = StatusCode::from_repr(line[..3].parse::<u16>().map_err(|_| {
                ConnError::InvalidFormat(format!("non-numeric response code: {:?}", &line[..3]))
            })?)
            .ok_or_else(|| {
                ConnError::InvalidFormat(format!("unknown status code: {}", &line[..3]))
            })?;

            // Verify all lines in a multi-line response share the same code.
            if let Some(prev) = first_code {
                if prev != parsed_code {
                    return Err(ConnError::InvalidFormat(format!(
                        "response code mismatch: {} vs {}",
                        prev, parsed_code
                    )));
                }
            } else {
                first_code = Some(parsed_code);
            }

            let sep = line.as_bytes()[3];
            let content = line[4..].to_string();

            match sep {
                b' ' => {
                    lines.push(content);
                    return Ok(Response {
                        code: parsed_code,
                        lines,
                    });
                }
                b'-' => {
                    lines.push(content);
                }
                // Data block: read until a line containing only "."
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
                other => {
                    return Err(ConnError::InvalidFormat(format!(
                        "unexpected separator byte: 0x{:02x}",
                        other
                    )));
                }
            }
        }
    }

    async fn authenticate_safe_cookie(mut self, cookie: &[u8]) -> Result<Self, ConnError> {
        let mut client_nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut client_nonce);

        self.send_command(
            Command::new("AUTHCHALLENGE")
                .arg("SAFECOOKIE")
                .arg(&hex::encode_upper(client_nonce)),
        )
        .await?;
        let resp = self.receive_response().await?;
        resp.expect_ok()?;

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

        self.send_command(Command::new("AUTHENTICATE").arg(&hex::encode_upper(client_hash)))
            .await?;
        let resp = self.receive_response().await?;
        resp.expect_ok()?;
        Ok(self)
    }
}

fn parse_protocol_info(lines: &[String]) -> Result<ProtocolInfo, ConnError> {
    if lines.is_empty() || lines[0] != "PROTOCOLINFO 1" {
        return Err(ConnError::InvalidFormat(
            "PROTOCOLINFO response missing header".into(),
        ));
    }
    if lines.last().map(|s| s.as_str()) != Some("OK") {
        return Err(ConnError::InvalidFormat(
            "PROTOCOLINFO response missing trailing OK".into(),
        ));
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
                cookie_file = Some(unquote_string_to_string(cookie_part)?);
            }
        } else if let Some(rest) = line.strip_prefix("VERSION Tor=") {
            version = Some(unquote_string_to_string(rest)?);
        }
    }

    if auth_methods.is_empty() {
        return Err(ConnError::InvalidFormat(
            "PROTOCOLINFO reports no auth methods".into(),
        ));
    }

    Ok(ProtocolInfo {
        version: version
            .ok_or_else(|| ConnError::InvalidFormat("PROTOCOLINFO missing VERSION".into()))?,
        auth_methods,
        cookie_file,
    })
}

fn parse_auth_challenge_response(lines: &[String]) -> Result<([u8; 32], [u8; 32]), ConnError> {
    if lines.len() != 1 {
        return Err(ConnError::InvalidFormat(format!(
            "AUTHCHALLENGE response has {} lines, expected 1",
            lines.len()
        )));
    }
    let line = &lines[0];
    let prefix = "AUTHCHALLENGE ";
    if !line.starts_with(prefix) {
        return Err(ConnError::InvalidFormat(format!(
            "AUTHCHALLENGE response missing prefix: {:?}",
            &line[..line.len().min(20)]
        )));
    }
    let rest = &line[prefix.len()..];
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() != 2 {
        return Err(ConnError::InvalidFormat(format!(
            "AUTHCHALLENGE response has {} parts, expected 2",
            parts.len()
        )));
    }

    let server_hash_hex = parts[0].strip_prefix("SERVERHASH=").ok_or_else(|| {
        ConnError::InvalidFormat(format!("missing SERVERHASH= prefix in {:?}", parts[0]))
    })?;
    let server_nonce_hex = parts[1].strip_prefix("SERVERNONCE=").ok_or_else(|| {
        ConnError::InvalidFormat(format!("missing SERVERNONCE= prefix in {:?}", parts[1]))
    })?;

    let server_hash = hex_to_array32(server_hash_hex)?;
    let server_nonce = hex_to_array32(server_nonce_hex)?;

    Ok((server_hash, server_nonce))
}

fn hex_to_array32(s: &str) -> Result<[u8; 32], ConnError> {
    let bytes = hex::decode(s)?;
    if bytes.len() != 32 {
        return Err(ConnError::InvalidFormat(format!(
            "expected 32-byte hash, got {} bytes",
            bytes.len()
        )));
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

/// Encodes a byte slice into a Tor control protocol compliant `QuotedString`.
///
/// Per the spec, only backslashes and double quotes are escaped; all other
/// octets are included verbatim. We intentionally do NOT use `\n`, `\t`,
/// `\r`, or octal escapes (`\0`...`\377`) here, because spec 2.1.1 says a
/// correct future QuotedString implementation will "never place a backslash
/// before a 'n', 't', 'r', or digit".
///
/// Although all current callers pass UTF-8 text, this function takes `&[u8]`
/// and returns `Vec<u8>` to stay faithful to the spec, which treats
/// QuotedString as a sequence of octets rather than a Unicode string.
///
/// Ref: <https://gitlab.torproject.org/tpo/core/torspec/-/raw/a6c90f44013f47d1c2dae8b4a5d25302e3b6e256/attic/text_formats/control-spec.txt>
/// Section: 2.1. Message format (Description format)
fn quote_string(s: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(s.len() + 2);
    result.push(b'"');
    for &b in s {
        match b {
            b'\\' => result.extend_from_slice(b"\\\\"),
            b'"' => result.extend_from_slice(b"\\\""),
            _ => result.push(b),
        }
    }
    result.push(b'"');
    result
}

/// Decodes a Tor control protocol `QuotedString` back into its original raw bytes.
///
/// This parser follows the future-proofing rules in the "Notes on an escaping
/// bug" section of the spec. Tor has historically emitted `CString` tokens (a
/// bug) instead of standard `QuotedString` tokens, using C-style escapes for
/// `\n`, `\t`, `\r`, and octal byte values `\0`...`\377`. To remain compatible
/// with both legacy Tor and future correct implementations, this parser:
///
/// - interprets `\n`, `\t`, `\r` and `\0`...`\377` as C escapes;
/// - treats a backslash followed by any other byte as that literal byte.
///
/// Ref: <https://gitlab.torproject.org/tpo/core/torspec/-/raw/a6c90f44013f47d1c2dae8b4a5d25302e3b6e256/attic/text_formats/control-spec.txt>
/// Section: 2.1.1. Message format (Notes on an escaping bug)
fn unquote_string(s: &[u8]) -> Result<Vec<u8>, ConnError> {
    if s.len() < 2 || s[0] != b'"' || s[s.len() - 1] != b'"' {
        return Err(ConnError::InvalidFormat(
            "QuotedString must be wrapped in double quotes".into(),
        ));
    }
    let mut bytes = Vec::new();
    let inner = &s[1..s.len() - 1];
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' {
            i += 1;
            if i >= inner.len() {
                return Err(ConnError::InvalidFormat(
                    "QuotedString ends with backslash".into(),
                ));
            }
            let c = inner[i];
            match c {
                b'n' => bytes.push(b'\n'),
                b't' => bytes.push(b'\t'),
                b'r' => bytes.push(b'\r'),
                b'0'..=b'7' => {
                    let mut value = (c - b'0') as u32;
                    let mut consumed = 1;
                    i += 1;
                    while consumed < 3 && i < inner.len() {
                        let next = inner[i];
                        if (b'0'..=b'7').contains(&next) {
                            let new_value = value * 8 + (next - b'0') as u32;
                            if new_value <= 255 {
                                value = new_value;
                                consumed += 1;
                                i += 1;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    bytes.push(value as u8);
                    continue;
                }
                _ => bytes.push(c),
            }
            i += 1;
        } else {
            bytes.push(inner[i]);
            i += 1;
        }
    }
    Ok(bytes)
}

/// Decodes a `QuotedString` and returns the result as a UTF-8 `String`.
///
/// This is a convenience wrapper around `unquote_string` for the common case
/// where the caller expects textual data (e.g. `COOKIEFILE`, `VERSION`, or
/// `GETINFO` values).
fn unquote_string_to_string(s: impl AsRef<[u8]>) -> Result<String, ConnError> {
    String::from_utf8(unquote_string(s.as_ref())?)
        .map_err(|e| ConnError::InvalidFormat(format!("QuotedString is not valid UTF-8: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_unquote() {
        let s = b"hello world";
        let quoted = quote_string(s);
        assert_eq!(quoted, b"\"hello world\"");
        assert_eq!(unquote_string(&quoted).unwrap(), s);

        let s = b"a\"b\\c\n";
        let quoted = quote_string(s);
        assert_eq!(quoted, b"\"a\\\"b\\\\c\n\"");
        assert_eq!(unquote_string(&quoted).unwrap(), s);

        let s: &[u8] = b"";
        let quoted = quote_string(s);
        assert_eq!(quoted, b"\"\"");
        assert_eq!(unquote_string(&quoted).unwrap(), s);

        // Control characters are included verbatim in a QuotedString.
        let s: &[u8] = b"\x00\x01\x02";
        let quoted = quote_string(s);
        assert_eq!(quoted, b"\"\x00\x01\x02\"");
        assert_eq!(unquote_string(&quoted).unwrap(), s);

        // Non-ASCII UTF-8 bytes are included verbatim and roundtrip.
        let s = "中文".as_bytes();
        let quoted = quote_string(s);
        assert_eq!(quoted, "\"中文\"".as_bytes());
        assert_eq!(unquote_string(&quoted).unwrap(), s);

        // C-style escapes from buggy/legacy Tor output are decoded.
        assert_eq!(unquote_string(b"\"a\\nb\"").unwrap(), b"a\nb");
        assert_eq!(unquote_string(b"\"a\\tb\"").unwrap(), b"a\tb");
        assert_eq!(unquote_string(b"\"a\\rb\"").unwrap(), b"a\rb");
        assert_eq!(unquote_string(b"\"a\\0b\"").unwrap(), b"a\0b");
        assert_eq!(unquote_string(b"\"a\\40b\"").unwrap(), b"a b");
        assert_eq!(unquote_string(b"\"a\\377b\"").unwrap(), b"a\xffb");
        assert_eq!(unquote_string(b"\"a\\\\b\"").unwrap(), b"a\\b");
        assert_eq!(unquote_string(b"\"a\\\"b\"").unwrap(), b"a\"b");
        assert_eq!(unquote_string(b"\"a\\xb\"").unwrap(), b"axb");

        assert_eq!(unquote_string(b"\"a\\00b\"").unwrap(), b"a\0b");
        assert_eq!(unquote_string(b"\"a\\000b\"").unwrap(), b"a\0b");
        assert_eq!(unquote_string(b"\"a\\400b\"").unwrap(), b"a 0b");
        assert_eq!(unquote_string(b"\"a\\0123\"").unwrap(), b"a\n3");

        assert_eq!(unquote_string(b"\"a\\z\\?\\k\"").unwrap(), b"az?k");

        assert!(unquote_string(b"").is_err());
        assert!(unquote_string(b"\"").is_err());
        assert!(unquote_string(b"hello").is_err());
        assert!(unquote_string(b"\"hello").is_err());
        assert!(unquote_string(b"\"hello\\\"").is_err());
        assert!(unquote_string(b"\"hello\\").is_err());

        assert_eq!(unquote_string(b"\"a\\\\b\"").unwrap(), b"a\\b");
        assert!(unquote_string(b"\"a\\").is_err());
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
