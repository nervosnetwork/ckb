use bigint::{H160, H256};
use core::{ProofPublicG, ProofPublickey, PublicKey};
use core::keygroup::KeyGroup;
use logger::Config as LogConfig;
use network::Config as NetworkConfig;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use toml;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub logger: LogConfig,
    pub miner_private_key: H160,
    pub signer_private_key: H256,
    pub key_pairs: Vec<KeyPair>,
    pub network: NetworkConfig,
    pub db_path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KeyPair {
    pub proof_public_key: ProofPublickey,
    pub proof_public_g: ProofPublicG,
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

#[cfg(test)]
mod tests {
    use super::{toml, Config, H160};
    use std::str::FromStr;

    #[test]
    fn config_deserialize() {
        let config: Config = toml::from_str(r#"
            miner_private_key = "0x1162b2f150c789aed32e2e0b3081dd6852926865"
            signer_private_key = "0x097c1a2d03b6c8fc270991051553219e34951b656382ca951c4c226d40a3b2d5"
            db_path = ".nervos/db"
            [logger]
            file = "/tmp/nervos.log"
            filter = "main=info,miner=info,chain=info"
            color = true
            [[key_pairs]]
            proof_public_key = "0x037151adfd9b0167d943ad816352bf0a96cfedfa15b60caddda91ad710732cb80cb54d46ce2fa9fc00"
            proof_public_g = "0x0ce477d8a5e6b27c9d8ec9e54efbf6f5b3455ffa01899a237d80d463f737f65f0d975a8bb3ceca9e00"
            signer_public_key = "0x1fd4decbd2ed6eb0f2d817eecffc26d1ff73b43b5d198a0bd2a973440f56d36d33e405525ecfb12215438a4bed517b9f8c3291dc80d03b51a8221caf983d703e"
            [[key_pairs]]
            proof_public_key = "0x03a02e943dc86d0d09a84f4c9c1d5ce0fbe4337c06fe1975e13fa1a7013f297701818d27400604d101"
            proof_public_g = "0x15f3b0f38a316392cb6829b18eb220bf9e40bef301869f275d27d35e176fcfaae839bc387021c7ce01"
            signer_public_key = "0xe3b72557d9bc2ed7f50a6f598dd6213c15eabb4b827d65ca32bf8d184c06c99fd76ab439b1cfe2c810bcdaa3890b5d4d1b816782cef0a37974ea0ec0a0f8bace"
            [[key_pairs]]
            proof_public_key = "0x133f84a2fba7d7d75c8bef53662ce555c034bdcc059765937f5b5fa3fad7fc5de96929f47285612001"
            proof_public_g = "0x004ca4aa1dcafbf4392cf395e8e78f3ebdc880ab0ef4a512f0dd486bf1a6861114f4d835d6dcacf001"
            signer_public_key = "0xa94942232300b74bf191e37435102a90cbe81b1ad5e2fccacfcd5aec115fd2b414d887b44641b3e3dc5fc137178cd1e027d104e7985aea9c539c46dd42dc2b9c"
            [[key_pairs]]
            proof_public_key = "0x07cafa7797efe36d26bb0af68bf8a55640f57fc811f5ee73bb7d10a2735cf0eb059b7cfb1107fc9d00"
            proof_public_g = "0x0318e21e32b26d6310e3609e78cdddfcc817f0f41a3deb02a611d63af17c7246b939360692bfd14900"
            signer_public_key = "0x223f2c5f71a9b3f42c65accc76ca90cd3a76f8587bf40f1069f3a6c05d1fbd645b04cfa45beaf884e2cf3b8d734aa7c6b68063eaa530f8fabf20c0341ae95156"
            [network]
            private_key = [0, 1, 2]
            public_key = [3, 4, 5]
            listen_addr = "/ip4/0.0.0.0/tcp/0"
            bootstrap_nodes = [["QmWvoPbu9AgEFLL5UyxpCfhxkLDd9T7zuerjhHiwsnqSh4", "/ip4/127.0.0.1/tcp/12345"]]
        "#).expect("Load config.");

        assert_eq!(true, config.logger_config().color);
        assert_eq!(
            H160::from_str("1162b2f150c789aed32e2e0b3081dd6852926865").unwrap(),
            config.miner_private_key
        );
        assert_eq!(4, config.key_group().len());
    }
}
