use std::io;
use std::str::ParseBoolError;

/// Uses 0, 64 - 113 as exit code.
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// Command line arguments error.
    Cli = 64,
    /// Config options error.
    Config = 65,
    /// Operation system I/O error.
    IO = 66,
    /// General application failures.
    Failure = 113,
}

impl ExitCode {
    /// Converts into signed 32-bit integer which can be used as the process exit status.
    pub fn into(self) -> i32 {
        self as i32
    }
}

impl From<io::Error> for ExitCode {
    fn from(err: io::Error) -> ExitCode {
        eprintln!("IO Error: {:?}", err);
        ExitCode::IO
    }
}

impl From<toml::de::Error> for ExitCode {
    fn from(err: toml::de::Error) -> ExitCode {
        eprintln!("Config Error: {:?}", err);
        ExitCode::Config
    }
}

impl From<ckb_logger::SetLoggerError> for ExitCode {
    fn from(err: ckb_logger::SetLoggerError) -> ExitCode {
        eprintln!("Config Error: {:?}", err);
        ExitCode::Config
    }
}

impl From<clap::Error> for ExitCode {
    fn from(err: clap::Error) -> ExitCode {
        eprintln!("Args Error: {:?}", err);
        ExitCode::Cli
    }
}

impl From<ParseBoolError> for ExitCode {
    fn from(err: ParseBoolError) -> ExitCode {
        eprintln!("Config Error: {:?}", err);
        ExitCode::Config
    }
}
