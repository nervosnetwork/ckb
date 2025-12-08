mod tor_basic;
mod tor_connect;
mod tor_hash_password;
mod tor_reconnect;
use ckb_async_runtime::Runtime;
use ckb_logger::{error, info};
use std::{path::Path, process::Child};
use tempfile::{TempDir, tempdir};
pub use tor_basic::*;
pub use tor_connect::*;
pub use tor_hash_password::*;
pub use tor_reconnect::*;

use crate::{global::obfs4proxy_binary, utils::find_available_port};

// Tor bridge:
const TOR_BRIDGES: &[&str] = &[
    "obfs4 [2605:6400:10:ea:fe01:dc20:ba03:4ff]:443 886CA31F71272FC8B3808C601FA3ABB8A2905DB4 cert=D+zypuFdMpP8riBUbInxIguzqClR0JKkP1DbkKz5es1+OP2Fao8jiXyM+B/+DYA2ZFy6UA iat-mode=0",
];

#[derive(Debug)]
struct TorServer {
    tor_command_path: String,
    socks_port: u16,
    control_port: u16,
    tor_process: Option<Child>,
    tor_data_dir: Option<TempDir>,
    controller_password: Option<String>,
}

impl Drop for TorServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TorServer {
    pub fn shutdown(&mut self) {
        if let Some(mut process) = self.tor_process.take() {
            process.kill().unwrap();

            match process.wait() {
                Ok(exit_status) => {
                    info!("wait tor process exit: {:?}", exit_status);
                }
                Err(err) => {
                    error!("wait tor process exit error: {:?}", err);
                }
            }
        }
    }
    pub fn tor_wait_bootstrap_done(&self) {
        let tor_controller_url = format!("127.0.0.1:{}", self.control_port);
        let controller_password = self.controller_password.clone();
        Runtime::new().unwrap().block_on(async {
            let tor_controller =
                ckb_onion::TorController::new(tor_controller_url, controller_password, None).await;
            let mut tor_controller = tor_controller.unwrap();
            if let Err(err) = tor_controller.wait_tor_server_bootstrap_done().await {
                error!("wait tor server bootstrap done error: {:?}", err);
            };
        });
    }
    pub fn new(controller_password: Option<String>) -> Self {
        let tor_command_path = std::option_env!("TOR_COMMAND_PATH")
            .unwrap_or("tor")
            .to_string();
        let mut tor_server = TorServer {
            tor_command_path,
            socks_port: find_available_port(),
            control_port: find_available_port(),
            tor_process: None,
            tor_data_dir: Some(tempdir().unwrap()),
            controller_password,
        };
        let tor_process = tor_server.tor_start(false);
        tor_server.tor_process = Some(tor_process);
        tor_server
    }

    fn tor_bridge_args(&self) -> Vec<String> {
        let mut bridges = Vec::new();
        for bridge in TOR_BRIDGES {
            bridges.push("--Bridge".to_string());
            bridges.push(bridge.to_string());
        }
        vec![
            "--UseBridges".to_string(),
            "1".to_string(),
            "--ClientTransportPlugin".to_string(),
            format!("obfs4 exec {}", obfs4proxy_binary().display()),
        ]
        .into_iter()
        .chain(bridges)
        .collect()
    }

    fn tor_hashed_control_password_args(&self) -> Vec<String> {
        if self.controller_password.is_none() {
            return vec![];
        }
        vec![
            "--HashedControlPassword".to_string(),
            self.tor_hashed_password(),
        ]
    }

    fn tor_base_args(&self, data_dir: &Path) -> Vec<String> {
        vec![
            "--SocksPort".to_string(),
            self.socks_port.to_string(),
            "--ControlPort".to_string(),
            self.control_port.to_string(),
            "--SafeLogging".to_string(),
            "0".to_string(),
            "--DataDirectory".to_string(),
            data_dir.display().to_string(),
        ]
    }

    fn build_tor_args(&self, data_dir: &Path) -> Vec<String> {
        let args: Vec<String> = self
            .tor_base_args(data_dir)
            .into_iter()
            .chain(self.tor_bridge_args())
            .chain(self.tor_hashed_control_password_args())
            .collect();
        info!("{}", args.join(" "));
        args
    }

    fn tor_start(&mut self, reuse_data_dir: bool) -> Child {
        let mut cmd = std::process::Command::new(&self.tor_command_path);

        if !reuse_data_dir {
            self.tor_data_dir = Some(tempdir().unwrap());
        }

        let tor_data_dir = self.tor_data_dir.as_ref().unwrap();

        let args = self.build_tor_args(tor_data_dir.path());
        let cmd = cmd.args(args);

        cmd.spawn().unwrap()
    }

    fn tor_hashed_password(&self) -> String {
        let mut cmd = std::process::Command::new(&self.tor_command_path);
        let password = self.controller_password.as_ref().unwrap();
        cmd.args(["--hash-password", password]);
        let output = cmd.output().unwrap().stdout;
        let hashed_password_untrim = match String::from_utf8(output) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to parse Tor hashed password output as UTF-8: {}", e);
                return String::new();
            }
        };

        let hashed_password = hashed_password_untrim.trim();
        info!("Got Tor hashed password: {}", hashed_password);
        hashed_password.to_string()
    }
}
