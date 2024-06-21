use ckb_app_config::{DaemonArgs, ExitCode};
use colored::*;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::io::Write;
use std::path::PathBuf;
use std::{fs, io};

pub fn daemon(args: DaemonArgs) -> Result<(), ExitCode> {
    let pid_file = &args.pid_file;
    if args.check {
        // find the pid file and check if the process is running
        match check_process(pid_file) {
            Ok(pid) => {
                eprintln!("{}, pid - {}", "ckb daemon service is running".green(), pid);
            }
            _ => {
                eprintln!("{}", "ckb daemon service is not running".red());
            }
        }
    } else if args.stop {
        kill_process(pid_file, "ckb")?;
        fs::remove_file(pid_file).map_err(|_| ExitCode::Failure)?;
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
    eprintln!(
        "stopping {} daemon service with pid {} ...",
        name,
        pid.to_string().red()
    );
    // Send a SIGTERM signal to the process
    let _ = kill(Pid::from_raw(pid), Some(Signal::SIGTERM)).map_err(|_| ExitCode::Failure);
    let mut wait_time = 60;
    eprintln!("{}", "waiting ckb service to stop ...".yellow());
    loop {
        let res = check_process(pid_file);
        match res {
            Ok(_) => {
                wait_time -= 1;
                eprint!("{}", ".".yellow());
                let _ = io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            _ if wait_time <= 0 => {
                eprintln!(
                    "{}",
                    format!(
                    "ckb daemon service is still running with pid {}..., stop it now forcefully ...",
                    pid
                )
                    .red()
                );
                kill(Pid::from_raw(pid), Some(Signal::SIGKILL)).map_err(|_| ExitCode::Failure)?;
                break;
            }
            _ => {
                break;
            }
        }
    }
    eprintln!("\n{}", "ckb daemon service stopped successfully".green());
    Ok(())
}
