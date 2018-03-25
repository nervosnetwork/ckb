use super::helper::HexSlice;
use super::template::{TemplatesExt, TEMPLATES};
use bigint::{H160, H256};
use bls;
use core::{ProofPublicG, ProofPublickey, PublicKey};
use crypto::secp::Generator;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use toml;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KeyPair {
    pub proof_public_key: ProofPublickey,
    pub proof_public_g: ProofPublicG,
    pub signer_public_key: PublicKey,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Signer {
    pub miner_private_key: H160,
    pub signer_private_key: H256,
    pub key_pairs: Vec<KeyPair>,
}

impl Signer {
    // temporary
    pub fn gen_and_print() {
        let (miner_private_key, proof_public_key, proof_public_g) = bls::key_gen();

        let gen = Generator::new();
        let (signer_private_key, signer_public_key) = gen.random_keypair()
            .expect("Generate random secp256k1 keypair");

        println!(
            "miner_private_key:    0x{}",
            HexSlice::new(&miner_private_key[..])
        );
        println!(
            "proof_public_key:    0x{}",
            HexSlice::new(&proof_public_key[..])
        );
        println!(
            "proof_public_g:    0x{}",
            HexSlice::new(&proof_public_g[..])
        );
        println!("signer_private_key:    0x{}", signer_private_key);
        println!("signer_public_key:    0x{}", signer_public_key);
    }
}

impl Default for Signer {
    fn default() -> Self {
        let (miner_private_key, proof_public_key, proof_public_g) = bls::key_gen();

        let gen = Generator::new();
        let (signer_private_key, signer_public_key) = gen.random_keypair()
            .expect("Generate random secp256k1 keypair");

        let key_pairs = vec![
            KeyPair {
                proof_public_key: ProofPublickey::from_slice(&proof_public_key[..]),
                proof_public_g: ProofPublicG::from_slice(&proof_public_g[..]),
                signer_public_key: signer_public_key.into(),
            },
        ];

        let miner_private_key = H160::from_slice(&miner_private_key[..]);

        Signer {
            miner_private_key,
            key_pairs,
            signer_private_key: signer_private_key.into(),
        }
    }
}

impl TemplatesExt for Signer {
    type Output = Signer;

    fn load<P: AsRef<Path>>(path: P) -> io::Result<Self::Output> {
        use std::error::Error;

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut config_string = String::new();
        reader.read_to_string(&mut config_string)?;
        toml::from_str(&config_string)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.description()))
    }

    fn write<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let content = TEMPLATES.render_signer(self);
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    fn load_or_write_default<P: AsRef<Path>>(path: P) -> io::Result<Self::Output> {
        match Self::load(path.as_ref()) {
            Ok(ret) => Ok(ret),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    let ret = Self::Output::default();
                    ret.write(path)?;
                    Ok(ret)
                } else {
                    Err(e)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{toml, H160, H256, Signer};
    use std::str::FromStr;

    #[test]
    fn signer_de() {
        let signer: Signer = toml::from_str(r#"
            miner_private_key = "0x1162b2f150c789aed32e2e0b3081dd6852926865"
            signer_private_key = "0x097c1a2d03b6c8fc270991051553219e34951b656382ca951c4c226d40a3b2d5"
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
        "#).expect("Spec deserialize.");

        assert_eq!(
            H160::from_str("1162b2f150c789aed32e2e0b3081dd6852926865").unwrap(),
            signer.miner_private_key
        );
        assert_eq!(
            H256::from_str("097c1a2d03b6c8fc270991051553219e34951b656382ca951c4c226d40a3b2d5")
                .unwrap(),
            signer.signer_private_key
        );
        assert_eq!(4, signer.key_pairs.len());
    }
}
