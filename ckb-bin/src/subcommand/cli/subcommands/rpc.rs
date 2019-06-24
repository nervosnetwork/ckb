use ckb_jsonrpc_types::{BlockNumber, CellOutPoint, EpochNumber, OutPoint, Unsigned};
use ckb_sdk::HttpRpcClient;
use clap::{App, Arg, ArgMatches, SubCommand};
use numext_fixed_hash::H256;

use super::CliSubCommand;
use crate::utils::arg_parser::{ArgParser, FixedHashParser, FromStrParser};
use crate::utils::printer::{OutputFormat, Printable};

pub struct RpcSubCommand<'a> {
    rpc_client: &'a mut HttpRpcClient,
}

impl<'a> RpcSubCommand<'a> {
    pub fn new(rpc_client: &'a mut HttpRpcClient) -> RpcSubCommand<'a> {
        RpcSubCommand { rpc_client }
    }

    pub fn subcommand() -> App<'static, 'static> {
        let arg_hash = Arg::with_name("hash")
            .long("hash")
            .takes_value(true)
            .validator(|input| FixedHashParser::<H256>::default().validate(input))
            .required(true);
        let arg_number = Arg::with_name("number")
            .long("number")
            .takes_value(true)
            .validator(|input| FromStrParser::<u64>::default().validate(input))
            .required(true)
            .help("Block number");

        SubCommand::with_name("rpc")
            .about("Invoke RPC call to node")
            .subcommands(vec![
                // [Chain]
                SubCommand::with_name("get_block")
                    .about("Get block content by hash")
                    .arg(arg_hash.clone().help("Block hash")),
                SubCommand::with_name("get_block_by_number")
                    .about("Get block content by block number")
                    .arg(arg_number.clone()),
                SubCommand::with_name("get_block_hash")
                    .about("Get block hash by block number")
                    .arg(arg_number.clone()),
                SubCommand::with_name("get_cells_by_lock_hash")
                    .about("Get cells by lock script hash")
                    .arg(arg_hash.clone().help("Lock hash"))
                    .arg(
                        Arg::with_name("from")
                            .long("from")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u64>::default().validate(input))
                            .required(true)
                            .help("From block number"),
                    )
                    .arg(
                        Arg::with_name("to")
                            .long("to")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u64>::default().validate(input))
                            .required(true)
                            .help("To block number"),
                    ),
                SubCommand::with_name("get_current_epoch").about("Get current epoch information"),
                SubCommand::with_name("get_epoch_by_number")
                    .about("Get epoch information by epoch number")
                    .arg(arg_number.clone().help("Epoch number")),
                SubCommand::with_name("get_live_cell")
                    .about("Get live cell (live means unspent)")
                    .arg(arg_hash.clone().required(false).help("Block hash"))
                    .arg(
                        Arg::with_name("tx-hash")
                            .long("tx-hash")
                            .takes_value(true)
                            .validator(|input| FixedHashParser::<H256>::default().validate(input))
                            .required(true)
                            .help("Tx hash"),
                    )
                    .arg(
                        Arg::with_name("index")
                            .long("index")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u32>::default().validate(input))
                            .required(true)
                            .help("Output index"),
                    ),
                SubCommand::with_name("get_tip_block_number").about("Get tip block number"),
                SubCommand::with_name("get_tip_header").about("Get tip header"),
                SubCommand::with_name("get_transaction")
                    .about("Get transaction content by transaction hash")
                    .arg(arg_hash.clone().help("Tx hash")),
                // [Net]
                SubCommand::with_name("get_peers").about("Get connected peers"),
                SubCommand::with_name("local_node_info").about("Get local node information"),
                // [Pool]
                SubCommand::with_name("tx_pool_info").about("Get transaction pool information"),
                // [`Stats`]
                SubCommand::with_name("get_blockchain_info").about("Get chain information"),
            ])
    }
}

impl<'a> CliSubCommand for RpcSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        match matches.subcommand() {
            ("get_block", Some(m)) => {
                let hash: H256 = FixedHashParser::<H256>::default().from_matches(m, "hash")?;

                let resp = self
                    .rpc_client
                    .get_block(hash)
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_block_by_number", Some(m)) => {
                let number: u64 = FromStrParser::<u64>::default().from_matches(m, "number")?;

                let resp = self
                    .rpc_client
                    .get_block_by_number(BlockNumber(number))
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_block_hash", Some(m)) => {
                let number = FromStrParser::<u64>::default().from_matches(m, "number")?;

                let resp = self
                    .rpc_client
                    .get_block_hash(BlockNumber(number))
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_cells_by_lock_hash", Some(m)) => {
                let lock_hash: H256 = FixedHashParser::<H256>::default().from_matches(m, "hash")?;
                let from_number: u64 = FromStrParser::<u64>::default().from_matches(m, "from")?;
                let to_number: u64 = FromStrParser::<u64>::default().from_matches(m, "to")?;

                let resp = self
                    .rpc_client
                    .get_cells_by_lock_hash(
                        lock_hash,
                        BlockNumber(from_number),
                        BlockNumber(to_number),
                    )
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_current_epoch", _) => {
                let resp = self
                    .rpc_client
                    .get_current_epoch()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_epoch_by_number", Some(m)) => {
                let number: u64 = FromStrParser::<u64>::default().from_matches(m, "number")?;
                let resp = self
                    .rpc_client
                    .get_epoch_by_number(EpochNumber(number))
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_live_cell", Some(m)) => {
                let block_hash: Option<H256> =
                    FixedHashParser::<H256>::default().from_matches_opt(m, "hash", false)?;

                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let index: u32 = FromStrParser::<u32>::default().from_matches(m, "index")?;
                let out_point = OutPoint {
                    cell: Some(CellOutPoint {
                        tx_hash,
                        index: Unsigned(u64::from(index)),
                    }),
                    block_hash,
                };

                let resp = self
                    .rpc_client
                    .get_live_cell(out_point)
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_tip_block_number", _) => {
                let resp = self
                    .rpc_client
                    .get_tip_block_number()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_tip_header", _) => {
                let resp = self
                    .rpc_client
                    .get_tip_header()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_transaction", Some(m)) => {
                let hash: H256 = FixedHashParser::<H256>::default().from_matches(m, "hash")?;

                let resp = self
                    .rpc_client
                    .get_transaction(hash)
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_blockchain_info", _) => {
                let resp = self
                    .rpc_client
                    .get_blockchain_info()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("local_node_info", _) => {
                let resp = self
                    .rpc_client
                    .local_node_info()
                    .call()
                    .map_err(|err| err.description().to_string())?;
                Ok(resp.render(format, color))
            }
            ("tx_pool_info", _) => {
                let resp = self
                    .rpc_client
                    .tx_pool_info()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("get_peers", _) => {
                let resp = self
                    .rpc_client
                    .get_peers()
                    .call()
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            _ => Err(matches.usage().to_owned()),
        }
    }
}
