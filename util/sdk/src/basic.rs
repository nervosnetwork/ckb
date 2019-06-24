use std::fmt;
use std::str::FromStr;

use bech32::{convert_bits, Bech32, ToBase32};
use bytes::Bytes;
use ckb_core::script::Script as CoreScript;
use ckb_crypto::secp::Pubkey;
use ckb_hash::blake2b_256;
use numext_fixed_hash::{H160, H256};
use serde_derive::{Deserialize, Serialize};

const PREFIX_MAINNET: &str = "ckb";
const PREFIX_TESTNET: &str = "ckt";
// \x01 is the P2PH version
const P2PH_MARK: &[u8] = b"\x01P2PH";

#[derive(Hash, Eq, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NetworkType {
    MainNet,
    TestNet,
}

impl NetworkType {
    pub fn from_prefix(value: &str) -> Option<NetworkType> {
        match value {
            PREFIX_MAINNET => Some(NetworkType::MainNet),
            PREFIX_TESTNET => Some(NetworkType::TestNet),
            _ => None,
        }
    }

    pub fn to_prefix(self) -> &'static str {
        match self {
            NetworkType::MainNet => PREFIX_MAINNET,
            NetworkType::TestNet => PREFIX_TESTNET,
        }
    }

    pub fn from_raw_str(value: &str) -> Option<NetworkType> {
        match value {
            "ckb" => Some(NetworkType::MainNet),
            "ckb_testnet" => Some(NetworkType::TestNet),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            NetworkType::MainNet => "ckb",
            NetworkType::TestNet => "ckb_testnet",
        }
    }
}

impl fmt::Display for NetworkType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.to_str())
    }
}

#[derive(Hash, Eq, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AddressFormat {
    // SECP256K1 algorithm	PK
    #[allow(dead_code)]
    SP2K,
    // SECP256R1 algorithm	PK
    #[allow(dead_code)]
    SP2R,
    // SECP256K1 + blake160	blake160(pk)
    P2PH,
    // Alias of SP2K	PK
    #[allow(dead_code)]
    P2PK,
}

impl Default for AddressFormat {
    fn default() -> AddressFormat {
        AddressFormat::P2PH
    }
}

impl AddressFormat {
    pub fn from_bytes(format: &[u8]) -> Result<AddressFormat, String> {
        match format {
            P2PH_MARK => Ok(AddressFormat::P2PH),
            _ => Err(format!("Unsupported address format data: {:?}", format)),
        }
    }

    pub fn to_bytes(self) -> Result<Vec<u8>, String> {
        match self {
            AddressFormat::P2PH => Ok(P2PH_MARK.to_vec()),
            _ => Err(format!("Unsupported address format: {:?}", self)),
        }
    }
}

#[derive(Hash, Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Address {
    format: AddressFormat,
    hash: H160,
}

impl Address {
    pub fn hash(&self) -> &H160 {
        &self.hash
    }

    pub fn lock_script(&self, code_hash: H256) -> CoreScript {
        CoreScript {
            args: vec![Bytes::from(self.hash.as_bytes())],
            code_hash,
        }
    }

    pub fn from_pubkey(format: AddressFormat, pubkey: &Pubkey) -> Result<Address, String> {
        if format != AddressFormat::P2PH {
            return Err("Only support P2PH for now".to_owned());
        }
        // Serialize pubkey as compressed format
        let hash = H160::from_slice(&blake2b_256(pubkey.serialize())[0..20])
            .expect("Generate hash(H160) from pubkey failed");
        Ok(Address { format, hash })
    }

    pub fn from_lock_arg(bytes: &[u8]) -> Result<Address, String> {
        let format = AddressFormat::P2PH;
        let hash = H160::from_slice(bytes).map_err(|err| err.to_string())?;
        Ok(Address { format, hash })
    }

    pub fn from_input(network: NetworkType, input: &str) -> Result<Address, String> {
        let value = Bech32::from_str(input).map_err(|err| err.to_string())?;
        if NetworkType::from_prefix(value.hrp())
            .filter(|input_network| input_network == &network)
            .is_none()
        {
            return Err(format!("Invalid hrp({}) for {}", value.hrp(), network));
        }
        let data = convert_bits(value.data(), 5, 8, false).unwrap();
        if data.len() != 25 {
            return Err(format!("Invalid input data length {}", data.len()));
        }
        let format = AddressFormat::from_bytes(&data[0..5])?;
        let hash = H160::from_slice(&data[5..25]).map_err(|err| err.to_string())?;
        Ok(Address { format, hash })
    }

    pub fn to_string(&self, network: NetworkType) -> String {
        let hrp = network.to_prefix();
        let mut data = [0; 25];
        let format_data = self.format.to_bytes().expect("Invalid address format");
        data[0..5].copy_from_slice(&format_data[0..5]);
        data[5..25].copy_from_slice(self.hash.as_fixed_bytes());
        let value = Bech32::new(hrp.to_string(), data.to_base32())
            .unwrap_or_else(|_| panic!("Encode address failed: hash={:?}", self.hash));
        format!("{}", value)
    }
}
