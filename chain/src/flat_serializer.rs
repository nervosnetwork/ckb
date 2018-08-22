use bincode::{
    deserialize as bincode_deserialize, serialize as bincode_serialize,
    serialized_size as bincode_serialized_size, ErrorKind, Result,
};
use serde::de::Deserialize;
use serde::ser::Serialize;

/// This module leverages bincode to build a new serializer with flat structure.
///
/// If we use bincode to serialize Vec<T>, it will create a series of bytes
/// which are essentially a black box for us. Even if we only need one of the
/// items within the Vec, we have to get the whole bytes, deserialize everything
/// and use the single item.
///
/// This flat serializer, on the other hand, will use bincode to serialize
/// each individual item separately, then it will simply concat all byte slices
/// to create a byte vector. With the generated Address indices in serialization,
/// we can then get a partial of all the data and deserialize individual item
/// separately.

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct Address {
    pub offset: usize,
    pub length: usize,
}

pub fn serialize<'a, T: Serialize + 'a>(
    values: impl Iterator<Item = &'a T>,
) -> Result<(Vec<u8>, Vec<Address>)> {
    values
        .map(|value| bincode_serialize(value))
        .collect::<Result<Vec<Vec<u8>>>>()
        .map(|serialized_values| {
            let serialized_sizes: Vec<usize> = serialized_values
                .iter()
                .map(|value| value.len() as usize)
                .collect();
            (
                serialized_values.concat(),
                generate_addresses_from_sizes(&serialized_sizes),
            )
        })
}

pub fn serialized_addresses<'a, T: Serialize + 'a>(
    values: impl Iterator<Item = &'a T>,
) -> Result<Vec<Address>> {
    values
        .map(|value| bincode_serialized_size(value).map(|len| len as usize))
        .collect::<Result<Vec<usize>>>()
        .map(|serialized_sizes| generate_addresses_from_sizes(&serialized_sizes))
}

pub fn deserialize<'a, T: Deserialize<'a>>(
    bytes: &'a [u8],
    addresses: &'a [Address],
) -> Result<Vec<T>> {
    addresses
        .iter()
        .map(
            |address| match bytes.get(address.offset..(address.offset + address.length)) {
                Some(sliced_bytes) => bincode_deserialize(sliced_bytes),
                None => Err(Box::new(ErrorKind::Custom(
                    "address is invalid!".to_string(),
                ))),
            },
        )
        .collect()
}

fn generate_addresses_from_sizes(sizes: &[usize]) -> Vec<Address> {
    let (_, addresses) = sizes.iter().fold(
        (0, Vec::with_capacity(sizes.len())),
        |(offset, mut addresses), size| {
            addresses.push(Address {
                offset,
                length: *size,
            });
            (offset + size, addresses)
        },
    );
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode::deserialize as bincode_deserialize;

    #[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
    struct Foobar {
        a: i64,
        b: String,
        c: usize,
    }

    #[test]
    fn serialize_and_deserialize_vector() {
        let items = vec![
            Foobar {
                a: 123,
                b: "this is a string".to_string(),
                c: 1000,
            },
            Foobar {
                a: -56,
                b: "Another string line".to_string(),
                c: 10,
            },
        ];
        let (data, addresses) = serialize(items.iter()).unwrap();
        let new_items = deserialize(&data, &addresses).unwrap();
        assert_eq!(items, new_items);
    }

    #[test]
    fn serialized_addresses_deserialize() {
        let items = vec![
            Foobar {
                a: 123,
                b: "this is a string".to_string(),
                c: 1000,
            },
            Foobar {
                a: -56,
                b: "Another string line".to_string(),
                c: 10,
            },
        ];
        let (_, addresses) = serialize(items.iter()).unwrap();
        let addresses2 = serialized_addresses(items.iter()).unwrap();
        assert_eq!(addresses, addresses2);
    }

    #[test]
    fn single_item_deserialize() {
        let items = vec![
            Foobar {
                a: 123,
                b: "this is a string".to_string(),
                c: 1000,
            },
            Foobar {
                a: -56,
                b: "Another string line".to_string(),
                c: 10,
            },
        ];
        let (data, addresses) = serialize(items.iter()).unwrap();

        let sliced_data = &data[addresses[1].offset..(addresses[1].offset + addresses[1].length)];
        let new_item = bincode_deserialize(sliced_data).unwrap();
        assert_eq!(items[1], new_item);
    }
}
