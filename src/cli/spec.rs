use super::template::{TemplatesExt, TEMPLATES};
use logger::Config as LogConfig;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use toml;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkConfig {
    pub listen_addr: String,
    pub bootstrap_nodes: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RpcConfig {
    pub listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Spec {
    pub logger: LogConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
}

impl Default for Spec {
    fn default() -> Self {
        let logger = LogConfig {
            filter: Some("main=info,miner=info,chain=info,network=debug,rpc=info".to_string()),
            color: true,
            file: Some("/tmp/nervos0.log".to_string()),
        };

        let network = NetworkConfig {
            listen_addr: "/ip4/0.0.0.0/tcp/0".to_string(),
            bootstrap_nodes: vec![
                (
                    "QmWvoPbu9AgEFLL5UyxpCfhxkLDd9T7zuerjhHiwsnqSh4".to_string(),
                    "/ip4/127.0.0.1/tcp/12345".to_string(),
                ),
            ],
        };

        let rpc = RpcConfig {
            listen_addr: "0.0.0.0:0".to_string(),
        };

        Spec {
            logger,
            network,
            rpc,
        }
    }
}

impl TemplatesExt for Spec {
    type Output = Spec;

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
        let content = TEMPLATES.render_spec(self);
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
    use super::{toml, Spec};

    #[test]
    fn spec_de() {
        let spec: Spec = toml::from_str(r#"
            [logger]
            file = "/tmp/nervos.log"
            filter = "main=info,miner=info,chain=info"
            color = true
            [network]
            listen_addr = "/ip4/0.0.0.0/tcp/0"
            bootstrap_nodes = [["QmWvoPbu9AgEFLL5UyxpCfhxkLDd9T7zuerjhHiwsnqSh4", "/ip4/127.0.0.1/tcp/12345"]]
            [rpc]
            listen_addr = "0.0.0.0:0"
        "#).expect("Spec deserialize.");

        assert_eq!(true, spec.logger.color);
    }
}
