mod tor_basic;
mod tor_connect;
mod tor_connect_normal;

use std::process::Child;

use ckb_logger::info;
pub use tor_basic::*;
pub use tor_connect::*;
pub use tor_connect_normal::*;

use crate::{global::obfs4proxy_binary, utils::find_available_port};

// Tor bridge:
// obfs4 46.226.104.16:23022 CE5E1921FD4CB84D40833C1CF68B0892135B9F04 cert=C/FVw98Zeeoayu7pJSfGwkwOFRtzk4sO20xd3XJtB3kTAuSYv3iXwmfcSkXDgeW3SLKwXw iat-mode=0
// obfs4 57.129.58.231:36884 9BF12EC5EADF1EF97078BB6E4E1CAA5041A38739 cert=GHa8FrPWOSqkdd7Y/rVQZue+gYnaFNJBXVOxBHE1WUSm/QIlxGNo25QS9kJ18OT8kDPccg iat-mode=0
const TOR_BRIDGES: &[&str] = &[
 "obfs4 46.226.104.16:23022 CE5E1921FD4CB84D40833C1CF68B0892135B9F04 cert=C/FVw98Zeeoayu7pJSfGwkwOFRtzk4sO20xd3XJtB3kTAuSYv3iXwmfcSkXDgeW3SLKwXw iat-mode=0",
     "obfs4 57.129.58.231:36884 9BF12EC5EADF1EF97078BB6E4E1CAA5041A38739 cert=GHa8FrPWOSqkdd7Y/rVQZue+gYnaFNJBXVOxBHE1WUSm/QIlxGNo25QS9kJ18OT8kDPccg iat-mode=0"];

#[derive(Clone, Debug)]
struct TorServer {
    tor_command_path: String,
    socks_port: u16,
    control_port: u16,
}

impl TorServer {
    pub fn new() -> Self {
        TorServer {
            tor_command_path: std::option_env!("TOR_COMMAND_PATH")
                .unwrap_or("tor")
                .to_string(),
            socks_port: find_available_port(),
            control_port: find_available_port(),
        }
    }

    fn build_tor_args(&self) -> Vec<String> {
        vec![
            "--SocksPort".to_string(),
            self.socks_port.to_string(),
            "--ControlPort".to_string(),
            self.control_port.to_string(),
            "--SafeLogging".to_string(),
            "0".to_string(),
            "--UseBridges".to_string(),
            "1".to_string(),
            "--ClientTransportPlugin".to_string(),
            format!("obfs4 exec {}", obfs4proxy_binary().display()),
            "--Bridge".to_string(),
            TOR_BRIDGES[0].to_string(),
            "--Bridge".to_string(),
            TOR_BRIDGES[1].to_string(),
        ]
    }

    fn tor_start(&self) -> Child {
        let mut cmd = std::process::Command::new(&self.tor_command_path);
        let cmd = cmd.args(self.build_tor_args());
        let child = cmd.spawn().unwrap();
        info!("tor started:({:?}) ; pid: {}", &self, child.id());
        child
    }
}
