pub mod account;
pub mod rpc;
#[cfg(unix)]
pub mod tui;
pub mod wallet;

pub mod local;

#[cfg(unix)]
pub use self::tui::TuiSubCommand;

pub use local::{
    LocalCellInputSubCommand, LocalCellSubCommand, LocalScriptSubCommand, LocalSubCommand,
    LocalTxSubCommand,
};

pub use account::AccountSubCommand;
pub use rpc::RpcSubCommand;
pub use wallet::{
    start_index_thread, IndexController, IndexRequest, IndexResponse, IndexThreadState,
    WalletSubCommand,
};

use clap::ArgMatches;

use crate::utils::printer::OutputFormat;

pub trait CliSubCommand {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String>;
}
