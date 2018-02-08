extern crate bincode;
#[cfg(test)]
#[macro_use]
extern crate serde_derive;

pub use bincode::{deserialize, serialize, Bounded, Infinite};

#[cfg(test)]
mod tests {
    extern crate bigint;
    use self::bigint::H256;
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Entity {
        x: H256,
        y: H256,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct World(Vec<Entity>);

    #[test]
    fn test_basic() {
        let world = World(vec![
            Entity {
                x: H256::default(),
                y: H256::default(),
            },
            Entity {
                x: H256::default(),
                y: H256::default(),
            },
        ]);

        let encoded: Vec<u8> = serialize(&world, Infinite).unwrap();

        let decoded: World = deserialize(&encoded[..]).unwrap();

        assert_eq!(world, decoded);
    }

}
