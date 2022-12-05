use crate::modify_config::{write_config, write_network_config};
use crate::run;
use std::path::{Path, PathBuf};
use std::process;

pub struct MainNode {
    pub binary_path: PathBuf,
    pub rpc_port: u64,
    pub p2p_port: u64,
    pub work_dir: PathBuf,
    pub bootnode: String,
    pub child: Option<process::Child>,
}

impl MainNode {
    pub fn validate(&self) {
        assert!(
            Path::new(&self.binary_path).exists(),
            "{:?} not exist",
            self.binary_path.clone()
        )
    }

    pub fn start(&mut self, main_node_log_filter: String) {
        run::ckb_init(
            self.binary_path.clone(),
            self.work_dir.clone(),
            self.rpc_port,
            self.p2p_port,
        );

        {
            let filepath = self.work_dir.join("ckb.toml");
            let mut bootnodes = toml_edit::Array::new();
            bootnodes.push(self.bootnode.clone());
            write_network_config(filepath, bootnodes);
            self.write_log_config(main_node_log_filter);
        }
        let child = self.ckb_run();
        self.child = Some(child);
    }
    pub fn ckb_run(&self) -> process::Child {
        process::Command::new(self.binary_path.clone())
            .arg("run")
            .arg("-C")
            .arg(self.work_dir.clone())
            .spawn()
            .unwrap()
    }

    pub fn write_log_config(&self, main_node_log_filter: String) {
        let filepath = self.work_dir.join("ckb.toml");
        write_config(filepath, |config| {
            config["logger"]["filter"] = toml_edit::value(&main_node_log_filter);
            config["logger"]["log_to_stdout"] = toml_edit::value(false);
        })
    }

    pub fn stop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.kill().unwrap();
            let _ = child.wait();
        }
    }
}
