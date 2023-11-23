use ckb_app_config::{DaemonArgs, ExitCode};
use colored::*;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::fs;
use std::path::PathBuf;

pub fn daemon(args: DaemonArgs) -> Result<(), ExitCode> {
    let name = "ckb";
    let pid_file = &args.pid_file;
    if args.check {
        // find the pid file and check if the process is running
        match check_process(pid_file) {
            Ok(pid) => {
                eprintln!("{:>10} : {:<12} pid - {}", name, "running".green(), pid);
            }
            _ => {
                eprintln!("{:>10} : {:<12}", name, "not running".red());
            }
        }
    } else if args.stop {
        kill_process(pid_file, "ckb")?;
        fs::remove_file(pid_file).map_err(|_| ExitCode::Failure)?;
    } else {
        unimplemented!()
    }
    Ok(())
}

pub fn check_process(pid_file: &PathBuf) -> Result<i32, ExitCode> {
    let pid_str = fs::read_to_string(pid_file).map_err(|_| ExitCode::Failure)?;
    let pid = pid_str
        .trim()
        .parse::<i32>()
        .map_err(|_| ExitCode::Failure)?;

    // Check if the process is running
    match kill(Pid::from_raw(pid), None) {
        Ok(_) => Ok(pid),
        Err(_) => Err(ExitCode::Failure),
    }
}

fn kill_process(pid_file: &PathBuf, name: &str) -> Result<(), ExitCode> {
    if check_process(pid_file).is_err() {
        eprintln!("{} is not running", name);
        return Ok(());
    }
    let pid_str = fs::read_to_string(pid_file).map_err(|_| ExitCode::Failure)?;
    let pid = pid_str
        .trim()
        .parse::<i32>()
        .map_err(|_| ExitCode::Failure)?;
    eprintln!("kill {} process {} ...", name, pid.to_string().red());
    // Send a SIGTERM signal to the process
    let _ = kill(Pid::from_raw(pid), Some(Signal::SIGTERM)).map_err(|_| ExitCode::Failure);
    // sleep 3 seconds and check if the process is still running
    std::thread::sleep(std::time::Duration::from_secs(3));
    match check_process(pid_file) {
        Ok(_) => kill(Pid::from_raw(pid), Some(Signal::SIGKILL)).map_err(|_| ExitCode::Failure),
        _ => Ok(()),
    }
}
