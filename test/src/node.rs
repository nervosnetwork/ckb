use fs_extra::dir::{copy, CopyOptions};
use std::fs::File;
use std::io::{Error, Read, Write};
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_CONFIG_FILE: &str = "default.json";

pub struct Node {
    binary: String,
    dir: String,
    p2p_port: u16,
    rpc_port: u16,
}

impl Node {
    pub fn new(binary: &str, dir: &str, p2p_port: u16, rpc_port: u16) -> Self {
        Self {
            binary: binary.to_string(),
            dir: dir.to_string(),
            p2p_port,
            rpc_port,
        }
    }

    pub fn start(&self) {
        self.init_config_file().expect("failed to init config file");
        let mut child = Command::new(self.binary.to_owned())
            .args(&["run", "-c", &format!("{}/{}", self.dir, DEFAULT_CONFIG_FILE)])
            .spawn()
            .expect("failed to run binary");
        child.wait().expect("failed to wait on child");
    }

    fn init_config_file(&self) -> Result<(), Error> {
        let mut options = CopyOptions::new();
        options.copy_inside = true;
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/nodes_template");
        let dest = PathBuf::from(&self.dir);
        copy(source, &dest, &options).expect("failed to copy template");

        let mut data = String::new();
        {
            let mut file = File::open(dest.join(DEFAULT_CONFIG_FILE))?;
            file.read_to_string(&mut data)?;
        }
        let new_data = data
            .replace("P2P_PORT", &self.p2p_port.to_string())
            .replace("RPC_PORT", &self.rpc_port.to_string());
        let mut file = File::create(dest.join(DEFAULT_CONFIG_FILE))?;
        file.write(new_data.as_bytes())?;
        Ok(())
    }
}
