use core::PublicKey;
use core::keygroup::KeyGroup;
use logger::Config as LogConfig;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use toml;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub logger: LogConfig,
    pub miner_private_key: Vec<u8>,
    pub signer_private_key: Vec<u8>,
    pub key_pairs: Vec<KeyPair>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KeyPair {
    pub proof_public_key: Vec<u8>,
    pub proof_public_g: Vec<u8>,
    pub signer_public_key: PublicKey,
}

impl Config {
    pub fn load(path: &str) -> Config {
        let file = File::open(path).unwrap();
        let mut reader = BufReader::new(file);
        let mut config_string = String::new();
        reader.read_to_string(&mut config_string).unwrap();
        toml::from_str(&config_string).unwrap()
    }

    pub fn logger_config(&self) -> LogConfig {
        self.logger.clone()
    }

    pub fn key_group(&self) -> KeyGroup {
        let mut kg = KeyGroup::default();
        for kp in self.key_pairs.clone() {
            kg.insert(kp.signer_public_key, kp.proof_public_key, kp.proof_public_g);
        }
        kg
    }
}
