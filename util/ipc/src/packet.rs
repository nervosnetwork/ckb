use crate::error::IpcError;
use crate::vlq::{vlq_decode_reader, vlq_encode};
use std::io::Read;

/// The `Packet` trait defines the interface for handling packets in an IPC context.
/// Types implementing this trait can be used to represent and manipulate packets.
///
/// # Required Methods
///
/// * `version` - This method returns the version of the packet.
/// * `payload` - This method returns a reference to the payload of the packet.
/// * `read_from` - This method reads a packet from a reader and returns an instance of the implementing type.
/// * `serialize` - This method serializes the packet into a vector of bytes.
pub trait Packet: Sized {
    /// Returns the version number of the packet.
    fn version(&self) -> u8;

    /// Returns a reference to the payload of the packet.
    fn payload(&self) -> &[u8];

    /// Reads a packet from a reader and returns an instance of the implementing type.
    fn read_from<R: Read>(reader: &mut R) -> Result<Self, IpcError>;

    /// Serializes the packet into a vector of bytes.
    fn serialize(&self) -> Vec<u8>;
}

/// A struct representing a request packet in IPC.
pub struct RequestPacket {
    version: u8,
    method_id: u64,
    payload: Vec<u8>,
}

impl Packet for RequestPacket {
    fn version(&self) -> u8 {
        self.version
    }

    fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn read_from<R: Read>(reader: &mut R) -> Result<Self, IpcError> {
        let version = vlq_decode_reader(reader)? as u8;
        let method_id = vlq_decode_reader(reader)?;
        let payload_length = vlq_decode_reader(reader)?;
        let mut payload = vec![0u8; payload_length as usize];
        reader
            .read_exact(&mut payload[..])
            .map_err(|_| IpcError::ReadExactError)?;
        Ok(RequestPacket {
            version,
            method_id,
            payload,
        })
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![];
        buf.extend_from_slice(&vlq_encode(self.version as u64));
        buf.extend_from_slice(&vlq_encode(self.method_id));
        buf.extend_from_slice(&vlq_encode(self.payload.len() as u64));
        buf.extend_from_slice(&self.payload);
        buf
    }
}

impl RequestPacket {
    /// Creates a new instance of RequestPacket with an payload.
    pub fn new(version: u8, method_id: u64, payload: Vec<u8>) -> Self {
        Self {
            version,
            method_id,
            payload,
        }
    }

    /// Returns the method ID of the packet.
    pub fn method_id(&self) -> u64 {
        self.method_id
    }
}

/// A struct representing a response packet in IPC.
pub struct ResponsePacket {
    version: u8,
    error_code: u64,
    payload: Vec<u8>,
}

impl Packet for ResponsePacket {
    fn version(&self) -> u8 {
        self.version
    }

    fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn read_from<R: Read>(reader: &mut R) -> Result<Self, IpcError> {
        let version = vlq_decode_reader(reader)? as u8;
        let error_code = vlq_decode_reader(reader)?;
        let payload_length = vlq_decode_reader(reader)?;
        let mut payload = vec![0u8; payload_length as usize];
        reader
            .read_exact(&mut payload[..])
            .map_err(|_| IpcError::ReadExactError)?;
        Ok(ResponsePacket {
            version,
            error_code,
            payload,
        })
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![];
        buf.extend_from_slice(&vlq_encode(self.version as u64));
        buf.extend_from_slice(&vlq_encode(self.error_code));
        buf.extend_from_slice(&vlq_encode(self.payload.len() as u64));
        buf.extend_from_slice(&self.payload);
        buf
    }
}

impl ResponsePacket {
    /// Creates a new instance of ResponsePacket with an payload.
    pub fn new(version: u8, error_code: u64, payload: Vec<u8>) -> Self {
        Self {
            version,
            error_code,
            payload,
        }
    }

    /// Returns the error code of the packet.
    pub fn error_code(&self) -> u64 {
        self.error_code
    }
}
