use super::rpc::Rpc;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::{fs, io};
use tempdir::TempDir;
use toml;
use toml::value::Table;

/// Struct representing a ckb node under test.
///
/// contains:
/// - state about the node
/// - a child process.Popen representing the running process
/// - an RPC client to the node
/// - P2P connections
/// To make things easier for the test writer.
pub struct TestNode {
    pub index: usize,
    pub base: TempDir,
    pub binary: PathBuf,
    pub config: Table,
    pub rpc: Rpc,
    process: Option<process::Child>,
}

fn write_config<P: AsRef<Path>>(config: &Table, path: P) -> io::Result<()> {
    fs::create_dir_all(path.as_ref()).expect("Unable to create dir");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .truncate(true)
        .open(path.as_ref().join("config.toml"))?;

    file.write_all(&toml::to_vec(config).expect("config toml invalid"))?;
    file.sync_all()?;
    Ok(())
}

impl TestNode {
    pub fn new(config: Table, index: usize, base: TempDir, binary: PathBuf) -> Self {
        // let datadir = format!("node{}", index);
        write_config(&config, base.path()).expect("failed to write config");

        let process = Some(
            process::Command::new(&binary)
                .env_clear()
                .args(&["run", "-d", base.path().to_str().unwrap()])
                .spawn()
                .expect("node failed to start"),
        );

        let rpc = Rpc::new(
            format!("http://{}", config["rpc"]["listen_addr"].as_str().unwrap())
                .parse()
                .expect("rpc bind addres"),
        );

        TestNode {
            index,
            base,
            binary,
            process,
            config,
            rpc,
        }
    }

    pub fn stop(&mut self) {
        self.process
            .take()
            .expect("node wasn't running")
            .kill()
            .expect("node wasn't running");
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        self.stop();
    }
}
