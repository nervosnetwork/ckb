use std::io;

/// Uses 0, 64 - 113 as exit code.
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// TODO(doc): @doitian
    Cli = 64,
    /// TODO(doc): @doitian
    Config = 65,
    /// TODO(doc): @doitian
    IO = 66,
    /// TODO(doc): @doitian
    Failure = 113,
}

impl ExitCode {
    /// TODO(doc): @doitian
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
