use super::helper::HexSlice;
use super::template::{TemplatesExt, TEMPLATES};
use bigint::H256;
use crypto::secp::Generator;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use toml;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Signer {
    pub private_key: H256,
}

impl Signer {
    // temporary
    pub fn gen_and_print() {
        let gen = Generator::new();
        let (private_key, public_key) = gen
            .random_keypair()
            .expect("Generate random secp256k1 keypair");

        println!("private_key:    0x{}", HexSlice::new(&private_key[..]));
        println!("public_key:    0x{}", HexSlice::new(&public_key[..]));
    }
}

impl Default for Signer {
    fn default() -> Self {
        let gen = Generator::new();
        let (private_key, _public_key) = gen
            .random_keypair()
            .expect("Generate random secp256k1 keypair");

        Signer {
            private_key: private_key.into(),
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
    use super::{toml, H256, Signer};
    use std::str::FromStr;

    #[test]
    fn signer_de() {
        let signer: Signer = toml::from_str(
            r#"
            private_key = "0x097c1a2d03b6c8fc270991051553219e34951b656382ca951c4c226d40a3b2d5"
        "#,
        ).expect("Spec deserialize.");

        assert_eq!(
            H256::from_str("097c1a2d03b6c8fc270991051553219e34951b656382ca951c4c226d40a3b2d5")
                .unwrap(),
            signer.private_key
        );
    }
}
