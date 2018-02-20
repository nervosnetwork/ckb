use bincode::{deserialize, serialize};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::result;
use std::sync::RwLock;

type Error = Box<ErrorKind>;
type Result<T> = result::Result<T, Error>;

#[derive(Debug)]
pub enum ErrorKind {
    DBError(String),
    SerializationError(String),
}

pub trait KeyValueDB {
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;

    fn write<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        match serialize(value) {
            Ok(serialized) => self.put(key, &serialized),
            Err(err) => Err(Box::new(ErrorKind::SerializationError(
                err.description().to_string(),
            ))),
        }
    }

    fn read<T>(&self, key: &[u8]) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        match self.get(key) {
            Ok(Some(ref value)) => match deserialize(value) {
                Ok(deserialized) => Ok(Some(deserialized)),
                Err(err) => Err(Box::new(ErrorKind::SerializationError(
                    err.description().to_string(),
                ))),
            },
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[derive(Default)]
pub struct MemoryKeyValueDB {
    hashmap: RwLock<HashMap<Vec<u8>, Vec<u8>>>,
}

impl KeyValueDB for MemoryKeyValueDB {
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut hashmap = self.hashmap.write().unwrap();
        hashmap.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let hashmap = self.hashmap.read().unwrap();
        if let Some(result) = hashmap.get(key) {
            Ok(Some(result.to_vec()))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Foo(u32);

    #[test]
    fn put_and_get() {
        let db = MemoryKeyValueDB::default();
        let key = &[0, 1, 2];
        let value = &[3, 4, 5];
        assert!(db.put(key, value).is_ok());
        assert_eq!(vec![3, 4, 5], db.get(key).unwrap().unwrap());
    }

    #[test]
    fn write_and_read() {
        let db = MemoryKeyValueDB::default();
        let key = &[0, 1, 2];
        let value = &Foo(345);
        assert!(db.write(key, value).is_ok());
        println!("db get {:?}", db.get(key));
        assert_eq!(Foo(345), db.read(key).unwrap().unwrap());
    }
}
