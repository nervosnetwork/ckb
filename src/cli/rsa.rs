use super::template::TemplatesExt;
use crypto::rsa::Rsa;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use toml;

impl TemplatesExt for Rsa {
    type Output = Rsa;

    fn load<P: AsRef<Path>>(path: P) -> io::Result<Self::Output> {
        use std::error::Error;

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut rsa_string = String::new();
        reader.read_to_string(&mut rsa_string)?;
        toml::from_str(&rsa_string)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.description()))
    }

    fn write<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let content =
            toml::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::Other, e.description()))?;
        file.write_all(&content)?;
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
