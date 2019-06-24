mod cell;
mod cell_input;
mod script;
mod tx;

pub use cell::LocalCellSubCommand;
pub use cell_input::LocalCellInputSubCommand;
pub use script::LocalScriptSubCommand;
pub use tx::LocalTxSubCommand;

use std::path::PathBuf;

use ckb_core::block::Block;
use ckb_jsonrpc_types::BlockNumber;
use ckb_sdk::{wallet::KeyStore, GenesisInfo, HttpRpcClient};
use clap::{App, ArgMatches, SubCommand};

use super::CliSubCommand;
use crate::utils::printer::{OutputFormat, Printable};

pub struct LocalSubCommand<'a> {
    rpc_client: &'a mut HttpRpcClient,
    key_store: &'a mut KeyStore,
    genesis_info: Option<GenesisInfo>,
    db_path: PathBuf,
}

impl<'a> LocalSubCommand<'a> {
    pub fn new(
        rpc_client: &'a mut HttpRpcClient,
        key_store: &'a mut KeyStore,
        genesis_info: Option<GenesisInfo>,
        db_path: PathBuf,
    ) -> LocalSubCommand<'a> {
        LocalSubCommand {
            rpc_client,
            key_store,
            genesis_info,
            db_path,
        }
    }

    fn genesis_info(&mut self) -> Result<GenesisInfo, String> {
        if self.genesis_info.is_none() {
            let genesis_block: Block = self
                .rpc_client
                .get_block_by_number(BlockNumber(0))
                .call()
                .map_err(|err| err.to_string())?
                .0
                .expect("Can not get genesis block?")
                .into();
            self.genesis_info = Some(GenesisInfo::from_block(&genesis_block)?);
        }
        Ok(self.genesis_info.clone().unwrap())
    }

    pub fn subcommand() -> App<'static, 'static> {
        SubCommand::with_name("local").subcommands(vec![
            LocalCellSubCommand::subcommand(),
            LocalCellInputSubCommand::subcommand(),
            LocalScriptSubCommand::subcommand(),
            LocalTxSubCommand::subcommand(),
            SubCommand::with_name("secp-dep"),
        ])
    }
}

impl<'a> CliSubCommand for LocalSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        match matches.subcommand() {
            ("script", Some(m)) => {
                LocalScriptSubCommand::new(self.rpc_client, self.db_path.clone())
                    .process(m, format, color)
            }
            ("cell", Some(m)) => LocalCellSubCommand::new(self.rpc_client, self.db_path.clone())
                .process(m, format, color),
            ("cell-input", Some(m)) => {
                LocalCellInputSubCommand::new(self.rpc_client, self.db_path.clone())
                    .process(m, format, color)
            }
            ("tx", Some(m)) => {
                let genesis_info = self.genesis_info()?;
                LocalTxSubCommand::new(
                    self.rpc_client,
                    self.key_store,
                    Some(genesis_info),
                    self.db_path.clone(),
                )
                .process(m, format, color)
            }
            ("secp-dep", _) => {
                let genesis_info = self.genesis_info()?;
                let result = serde_json::json!({
                    "out_point": genesis_info.secp_dep(),
                    "code_hash": genesis_info.secp_code_hash(),
                });
                Ok(result.render(format, color))
            }
            _ => Err(matches.usage().to_owned()),
        }
    }
}
