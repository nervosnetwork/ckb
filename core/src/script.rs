use bigint::H256;
use hash::sha3_256;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub version: u8,
    pub arguments: Vec<Vec<u8>>,
    pub redeem_script: Vec<u8>,
}

impl Script {
    pub fn new(version: u8, arguments: Vec<Vec<u8>>, redeem_script: Vec<u8>) -> Self {
        Script {
            version,
            arguments,
            redeem_script,
        }
    }

    pub fn redeem_script_hash(&self) -> H256 {
        match self.version {
            0 => sha3_256(&self.redeem_script).into(),
            _ => H256::from(0),
        }
    }
}

// impl From<&'static str> for Script {
// 	fn from(s: &'static str) -> Self {
// 		Script::new(s.into())
// 	}
// }

// impl From<Bytes> for Script {
// 	fn from(s: Bytes) -> Self {
// 		Script::new(s)
// 	}
// }

// impl From<Vec<u8>> for Script {
// 	fn from(v: Vec<u8>) -> Self {
// 		Script::new(v.into())
// 	}
// }

// impl From<Script> for Bytes {
// 	fn from(script: Script) -> Self {
// 		script.data
// 	}
// }
