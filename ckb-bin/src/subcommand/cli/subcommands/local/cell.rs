use std::fs;
use std::io::Read;
use std::path::PathBuf;

use bytes::Bytes;
use ckb_core::{
    transaction::{CellOutPoint, CellOutput},
    Capacity,
};
use ckb_jsonrpc_types::{CellOutPoint as RpcCellOutPoint, CellOutput as RpcCellOutput};
use ckb_sdk::{
    to_local_cell_out_point, with_rocksdb, CellAliasManager, CellManager, HttpRpcClient,
    ScriptManager,
};
use clap::{App, Arg, ArgMatches, SubCommand};
use numext_fixed_hash::H256;

use super::super::CliSubCommand;
use crate::utils::arg_parser::{
    ArgParser, CapacityParser, CellOutPointParser, FilePathParser, FixedHashParser, HexParser,
};
use crate::utils::printer::{OutputFormat, Printable};

pub struct LocalCellSubCommand<'a> {
    _rpc_client: &'a mut HttpRpcClient,
    db_path: PathBuf,
}

impl<'a> LocalCellSubCommand<'a> {
    pub fn new(rpc_client: &'a mut HttpRpcClient, db_path: PathBuf) -> LocalCellSubCommand<'a> {
        LocalCellSubCommand {
            _rpc_client: rpc_client,
            db_path,
        }
    }

    pub fn subcommand() -> App<'static, 'static> {
        let arg_name = Arg::with_name("name")
            .long("name")
            .takes_value(true)
            .required(true)
            .help("Cell name");
        let arg_json_path = Arg::with_name("path")
            .long("path")
            .takes_value(true)
            .validator(|input| FilePathParser::new(false).validate(input))
            .required(true)
            .help("JSON file path");
        SubCommand::with_name("cell")
            .about("Local cell management")
            .subcommands(vec![
                SubCommand::with_name("add")
                    .arg(arg_name.clone())
                    .arg(
                        Arg::with_name("data-path")
                            .long("data-path")
                            .takes_value(true)
                            .validator(|input| FilePathParser::new(true).validate(input))
                            .help("Data file path"),
                    )
                    .arg(
                        Arg::with_name("data")
                            .long("data")
                            .takes_value(true)
                            .validator(|input| HexParser.validate(input))
                            .help("Hex data"),
                    )
                    .arg(
                        Arg::with_name("lock-hash")
                            .long("lock-hash")
                            .takes_value(true)
                            .validator(|input| FixedHashParser::<H256>::default().validate(input))
                            .required(true)
                            .help("Lock script hash"),
                    )
                    .arg(
                        Arg::with_name("type-hash")
                            .long("type-hash")
                            .takes_value(true)
                            .validator(|input| FixedHashParser::<H256>::default().validate(input))
                            .help("Type script hash"),
                    )
                    .arg(
                        Arg::with_name("alias")
                            .long("alias")
                            .takes_value(true)
                            .validator(|input| CellOutPointParser.validate(input))
                            .help("Alias to a cell out point (for reference cell already dead on chain)")
                    )
                    .arg(
                        Arg::with_name("capacity")
                            .long("capacity")
                            .takes_value(true)
                            .validator(|input| CapacityParser.validate(input))
                            .help("Capacity (unit: CKB, format: 123.456)"),
                    ),
                SubCommand::with_name("remove").arg(arg_name.clone()),
                SubCommand::with_name("show").arg(arg_name.clone()),
                SubCommand::with_name("alias")
                    .arg(arg_name.clone())
                    .arg(
                        Arg::with_name("out-point")
                            .long("out-point")
                            .validator(|input| CellOutPointParser.validate(input))
                            .help("Alias by cell out pointer")
                    ),
                SubCommand::with_name("list"),
                SubCommand::with_name("load")
                    .arg(arg_name.clone())
                    .arg(arg_json_path.clone()),
                SubCommand::with_name("dump")
                    .arg(arg_name.clone())
                    .arg(arg_json_path.clone()),
            ])
    }
}

impl<'a> CliSubCommand for LocalCellSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        let cell_json = |cell, name: &str, alias| {
            let cell_out_point: RpcCellOutPoint = to_local_cell_out_point(name).into();
            serde_json::json!({
                "cell": cell,
                "local_cell_out_point": cell_out_point,
                "name": name,
                "alias": alias,
            })
        };
        match matches.subcommand() {
            ("add", Some(m)) => {
                let name: String = m.value_of("name").unwrap().to_owned();
                let data_path: Option<String> = m.value_of("data-path").map(ToOwned::to_owned);
                let data_bin: Option<Vec<u8>> = HexParser.from_matches_opt(m, "data", false)?;
                let lock_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "lock-hash")?;
                let type_hash: Option<H256> =
                    FixedHashParser::<H256>::default().from_matches_opt(m, "type-hash", false)?;
                let alias: Option<CellOutPoint> =
                    CellOutPointParser.from_matches_opt(m, "alias", false)?;
                let capacity: u64 = CapacityParser.from_matches(m, "capacity")?;

                let mut data = Vec::new();
                if let Some(path) = data_path {
                    let mut file = fs::File::open(path).map_err(|err| err.to_string())?;
                    file.read_to_end(&mut data).map_err(|err| err.to_string())?;
                }
                if let Some(data_bin) = data_bin {
                    data = data_bin;
                }
                let data = Bytes::from(data);

                let lock = with_rocksdb(&self.db_path, None, |db| {
                    ScriptManager::new(db).get(&lock_hash).map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let type_ = match type_hash {
                    Some(hash) => Some(
                        with_rocksdb(&self.db_path, None, |db| {
                            ScriptManager::new(db).get(&hash).map_err(Into::into)
                        })
                        .map_err(|err| format!("{:?}", err))?,
                    ),
                    None => None,
                };

                let cell_output = CellOutput {
                    capacity: Capacity::shannons(capacity),
                    data,
                    lock,
                    type_,
                };
                with_rocksdb(&self.db_path, None, |db| {
                    if let Some(ref cell_out_point) = alias {
                        CellAliasManager::new(db).add(cell_out_point, &name).ok();
                    }
                    CellManager::new(db)
                        .add(&name, cell_output.clone())
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;

                let rpc_cell: RpcCellOutput = cell_output.into();
                let resp = cell_json(rpc_cell, &name, alias);
                Ok(resp.render(format, color))
            }
            ("remove", Some(m)) => {
                let name: String = m.value_of("name").map(ToOwned::to_owned).unwrap();
                let cell_output = with_rocksdb(&self.db_path, None, |db| {
                    let alias = CellAliasManager::new(db).get_by_name(&name).ok();
                    CellManager::new(db)
                        .remove(&name)
                        .map(|cell| {
                            let cell: RpcCellOutput = cell.into();
                            cell_json(cell, &name, alias)
                        })
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                Ok(cell_output.render(format, color))
            }
            ("show", Some(m)) => {
                let name: String = m.value_of("name").map(ToOwned::to_owned).unwrap();
                let cell_output = with_rocksdb(&self.db_path, None, |db| {
                    let alias = CellAliasManager::new(db).get_by_name(&name).ok();
                    CellManager::new(db)
                        .get(&name)
                        .map(|cell| {
                            let cell: RpcCellOutput = cell.into();
                            cell_json(cell, &name, alias)
                        })
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                Ok(cell_output.render(format, color))
            }
            ("alias", Some(m)) => {
                let name: String = m.value_of("name").map(ToOwned::to_owned).unwrap();
                let alias: CellOutPoint = CellOutPointParser.from_matches(m, "out-point")?;
                let cell_output = with_rocksdb(&self.db_path, None, |db| {
                    CellAliasManager::new(db).add(&alias, &name)?;
                    CellManager::new(db)
                        .get(&name)
                        .map(|cell| {
                            let cell: RpcCellOutput = cell.into();
                            cell_json(cell, &name, Some(alias))
                        })
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                Ok(cell_output.render(format, color))
            }
            ("list", _) => {
                let cells = with_rocksdb(&self.db_path, None, |db| {
                    CellManager::new(db)
                        .list()
                        .map(|cells| {
                            cells
                                .into_iter()
                                .map(|(name, cell)| {
                                    let alias = CellAliasManager::new(db).get_by_name(&name).ok();
                                    let cell: RpcCellOutput = cell.into();
                                    cell_json(cell, &name, alias)
                                })
                                .collect::<Vec<_>>()
                        })
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                Ok(cells.render(format, color))
            }
            ("dump", Some(_m)) => Ok("null".to_string()),
            ("load", Some(_m)) => Ok("null".to_string()),
            _ => Err(matches.usage().to_owned()),
        }
    }
}
