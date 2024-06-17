pub mod ckb_protocol_ctx;

use ckb_network::Flags;
use ckb_network::PeerId;

pub struct BufManager<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> BufManager<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            buf: data,
            offset: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn get_buf(&mut self, len: usize) -> Vec<u8> {
        let buf_len = self.buf.len();
        if buf_len >= self.offset + len && self.offset != buf_len {
            let r = self.buf[self.offset..self.offset + len].to_vec();
            self.offset += len;
            r
        } else {
            let mut r = Vec::<u8>::with_capacity(len);
            r.resize(len, 0);
            r[0..(buf_len - self.offset)].copy_from_slice(&self.buf[self.offset..]);
            self.offset = buf_len;
            r
        }
    }

    pub fn get<T: FromBytes<T>>(&mut self) -> T {
        T::from_bytes(&self.get_buf(T::type_size()))
    }

    pub fn other(&mut self) -> Vec<u8> {
        if self.is_end() {
            return Vec::new();
        }

        self.buf[self.offset..].to_vec()
    }

    pub fn is_end(&self) -> bool {
        self.offset >= self.buf.len()
    }
}

pub trait FromBytes<T> {
    fn from_bytes(d: &[u8]) -> T;
    fn type_size() -> usize;
}

impl FromBytes<u8> for u8 {
    fn type_size() -> usize {
        1
    }
    fn from_bytes(d: &[u8]) -> u8 {
        u8::from_le_bytes(d.try_into().unwrap())
    }
}
impl FromBytes<u16> for u16 {
    fn type_size() -> usize {
        2
    }
    fn from_bytes(d: &[u8]) -> u16 {
        u16::from_le_bytes(d.try_into().unwrap())
    }
}
impl FromBytes<u32> for u32 {
    fn type_size() -> usize {
        4
    }
    fn from_bytes(d: &[u8]) -> u32 {
        u32::from_le_bytes(d.try_into().unwrap())
    }
}
impl FromBytes<u64> for u64 {
    fn type_size() -> usize {
        8
    }
    fn from_bytes(d: &[u8]) -> u64 {
        u64::from_le_bytes(d.try_into().unwrap())
    }
}
impl FromBytes<u128> for u128 {
    fn type_size() -> usize {
        8
    }
    fn from_bytes(d: &[u8]) -> u128 {
        u128::from_le_bytes(d.try_into().unwrap())
    }
}
impl FromBytes<usize> for usize {
    fn type_size() -> usize {
        std::mem::size_of::<usize>()
    }
    fn from_bytes(d: &[u8]) -> usize {
        usize::from_le_bytes(d.try_into().unwrap())
    }
}

// fuzz_peer_store
impl FromBytes<Flags> for Flags {
    fn type_size() -> usize {
        1
    }
    fn from_bytes(d: &[u8]) -> Flags {
        unsafe {
            Flags::from_bits_unchecked(
                (u8::from_le_bytes(d.try_into().unwrap()) % 0b1000000) as u64,
            )
        }
    }
}
impl FromBytes<std::net::Ipv4Addr> for std::net::Ipv4Addr {
    fn type_size() -> usize {
        4
    }
    fn from_bytes(d: &[u8]) -> std::net::Ipv4Addr {
        std::net::Ipv4Addr::from(u32::from_bytes(d))
    }
}
impl FromBytes<std::net::Ipv6Addr> for std::net::Ipv6Addr {
    fn type_size() -> usize {
        16
    }
    fn from_bytes(d: &[u8]) -> std::net::Ipv6Addr {
        std::net::Ipv6Addr::from(u128::from_bytes(d))
    }
}
impl FromBytes<ipnetwork::Ipv4Network> for ipnetwork::Ipv4Network {
    fn type_size() -> usize {
        4
    }
    fn from_bytes(d: &[u8]) -> ipnetwork::Ipv4Network {
        Self::from(std::net::Ipv4Addr::from_bytes(d))
    }
}
impl FromBytes<ipnetwork::Ipv6Network> for ipnetwork::Ipv6Network {
    fn type_size() -> usize {
        16
    }
    fn from_bytes(d: &[u8]) -> ipnetwork::Ipv6Network {
        Self::from(std::net::Ipv6Addr::from_bytes(d))
    }
}
impl FromBytes<PeerId> for PeerId {
    fn type_size() -> usize {
        32
    }
    fn from_bytes(d: &[u8]) -> PeerId {
        PeerId::from_bytes(vec![vec![0x12], vec![0x20], d.to_vec()].concat()).unwrap()
    }
}
