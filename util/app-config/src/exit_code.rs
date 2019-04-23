use std::io;

/// Uses 0, 64 - 113 as exit code.
#[repr(i32)]
#[derive(Copy, Clone)]
pub enum ExitCode {
    Cli = 64,
    Config = 65,
    IO = 66,
    Failure = 113,
}

impl ExitCode {
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

impl From<log::SetLoggerError> for ExitCode {
    fn from(err: log::SetLoggerError) -> ExitCode {
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
