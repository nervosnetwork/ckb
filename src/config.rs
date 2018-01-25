use logger::Config as LogConfig;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use toml;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(rename = "log")]
    pub logger: LogConfig,
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
}
