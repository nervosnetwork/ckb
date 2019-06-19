//! Canonical Serializer
//! see Readme for details.
//!

use bytes::Bytes;
use failure::{ensure, Error};
use numext_fixed_hash::H256;
use std::io::Write;

pub type Result<T> = ::std::result::Result<T, Error>;

pub trait CanonicalSerialize {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()>;
}

pub struct CanonicalSerializer<W> {
    buf: W,
}

impl<W: Write> CanonicalSerializer<W> {
    pub fn new(buf: W) -> Self {
        CanonicalSerializer { buf }
    }

    pub fn encode_u8(&mut self, v: u8) -> Result<&mut Self> {
        self.buf.write_all(&[v])?;
        Ok(self)
    }

    pub fn encode_u32(&mut self, v: u32) -> Result<&mut Self> {
        self.buf.write_all(&v.to_le_bytes())?;
        Ok(self)
    }

    pub fn encode_u64(&mut self, v: u64) -> Result<&mut Self> {
        self.buf.write_all(&v.to_le_bytes())?;
        Ok(self)
    }

    pub fn encode_h160(&mut self, v: &[u8]) -> Result<&mut Self> {
        ensure!(
            v.len() == 20,
            "serialize H160 length error expect 20, got {}",
            v.len()
        );
        self.buf.write_all(v)?;
        Ok(self)
    }

    pub fn encode_h256(&mut self, v: &[u8]) -> Result<&mut Self> {
        ensure!(
            v.len() == 32,
            "serialize H256 length error expect 32, got {}",
            v.len()
        );
        self.buf.write_all(v)?;
        Ok(self)
    }

    pub fn encode_u256(&mut self, v: &[u8]) -> Result<&mut Self> {
        ensure!(
            v.len() == 32,
            "serialize U256 length error expect 32, got {}",
            v.len()
        );
        self.buf.write_all(v)?;
        Ok(self)
    }

    pub fn encode_fix_length_bytes(&mut self, v: &[u8], len: usize) -> Result<&mut Self> {
        ensure!(
            v.len() == len,
            "serialize fix length bytes error expect {}, got {}",
            len,
            v.len()
        );
        self.buf.write_all(v)?;
        Ok(self)
    }

    pub fn encode_bytes(&mut self, v: &[u8]) -> Result<&mut Self> {
        self.encode_u32(v.len() as u32)?;
        self.buf.write_all(v)?;
        Ok(self)
    }

    pub fn encode_vec<T: CanonicalSerialize>(&mut self, list: &[T]) -> Result<&mut Self> {
        self.encode_u32(list.len() as u32)?;
        for elem in list {
            elem.serialize(self)?;
        }
        Ok(self)
    }

    pub fn encode_struct<T: CanonicalSerialize>(&mut self, s: T) -> Result<&mut Self> {
        s.serialize(self)?;
        Ok(self)
    }

    pub fn encode_struct_ref<T: CanonicalSerialize>(&mut self, s: &T) -> Result<&mut Self> {
        s.serialize(self)?;
        Ok(self)
    }

    pub fn encode_option<T: CanonicalSerialize>(&mut self, item: Option<T>) -> Result<&mut Self> {
        self.encode_option_ref(&item)
    }

    pub fn encode_option_ref<T: CanonicalSerialize>(
        &mut self,
        item: &Option<T>,
    ) -> Result<&mut Self> {
        match item {
            Some(item) => self.encode_u8(1)?.encode_struct_ref(item),
            None => self.encode_u8(0),
        }
    }
}

// implement basic types
impl CanonicalSerialize for u8 {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_u8(*self)?;
        Ok(())
    }
}

impl CanonicalSerialize for u32 {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_u32(*self)?;
        Ok(())
    }
}

impl CanonicalSerialize for u64 {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_u64(*self)?;
        Ok(())
    }
}

impl CanonicalSerialize for H256 {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_h256(self.as_bytes())?;
        Ok(())
    }
}

impl CanonicalSerialize for Bytes {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_bytes(self)?;
        Ok(())
    }
}

impl<T: CanonicalSerialize> CanonicalSerialize for Vec<T> {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
        serializer.encode_vec(self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_serialize_bytes() {
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer.encode_bytes(b"hello world").unwrap();
        assert_eq!(buf, b"\x0b\x00\x00\x00hello world");
    }

    #[test]
    fn test_encode_vec() {
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer.encode_vec(&[1u8, 2, 3]).unwrap();
        assert_eq!(buf, b"\x03\x00\x00\x00\x01\x02\x03");

        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer.encode_vec(&[1u32, 2, 3]).unwrap();
        assert_eq!(
            buf,
            b"\x03\x00\x00\x00\x01\x00\x00\x00\x02\x00\x00\x00\x03\x00\x00\x00"
        );
    }

    #[test]
    fn test_encode_empty_vec() {
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer.encode_vec::<u64>(&[]).unwrap();
        assert_eq!(buf, b"\x00\x00\x00\x00");
    }

    #[test]
    fn test_encode_vec_of_bytes() {
        struct Bytes<'a>(&'a [u8]);
        impl<'a> CanonicalSerialize for Bytes<'a> {
            fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
                serializer.encode_bytes(&self.0)?;
                Ok(())
            }
        }
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer
            .encode_vec(&[Bytes(b"hello"), Bytes(b"world"), Bytes(b"blockchain")])
            .unwrap();
        assert_eq!(
            &buf[..],
            &b"\x03\x00\x00\x00\x05\x00\x00\x00hello\x05\x00\x00\x00world\n\x00\x00\x00blockchain"
                [..]
        );
    }

    #[test]
    fn test_encode_uint() {
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer
            .encode_u8(1)
            .unwrap()
            .encode_u32(2)
            .unwrap()
            .encode_u64(3)
            .unwrap();
        assert_eq!(buf, b"\x01\x02\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00");
    }

    #[test]
    fn test_encode_option() {
        struct OptionU8(Option<u8>);
        impl CanonicalSerialize for OptionU8 {
            fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> Result<()> {
                serializer.encode_option(self.0)?;
                Ok(())
            }
        }
        let mut buf = Vec::new();
        let mut serializer = CanonicalSerializer::new(&mut buf);
        serializer
            .encode_vec(&[OptionU8(Some(1u8)), OptionU8(None), OptionU8(Some(3))])
            .unwrap();
        assert_eq!(buf, b"\x03\x00\x00\x00\x01\x01\x00\x01\x03");
    }
}
