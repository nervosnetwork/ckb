use std::path::PathBuf;

use bytes::Bytes;
use ckb_core::{
    block::Block,
    transaction::{CellInput, CellOutPoint, CellOutput, OutPoint, TransactionBuilder, Witness},
};
use ckb_jsonrpc_types::{BlockNumber, TransactionView};
use ckb_sdk::{
    wallet::KeyStore, with_rocksdb, CellInputManager, CellManager, GenesisInfo, HttpRpcClient,
    TransactionManager,
};
use clap::{App, Arg, ArgMatches, SubCommand};
use numext_fixed_hash::H256;

use super::super::CliSubCommand;
use crate::utils::arg_parser::{
    ArgParser, CellOutPointParser, EitherParser, EitherValue, FixedHashParser, FromStrParser,
    HexParser, NullParser,
};
use crate::utils::printer::{OutputFormat, Printable};

pub struct LocalTxSubCommand<'a> {
    rpc_client: &'a mut HttpRpcClient,
    key_store: &'a mut KeyStore,
    genesis_info: Option<GenesisInfo>,
    db_path: PathBuf,
}

impl<'a> LocalTxSubCommand<'a> {
    pub fn new(
        rpc_client: &'a mut HttpRpcClient,
        key_store: &'a mut KeyStore,
        genesis_info: Option<GenesisInfo>,
        db_path: PathBuf,
    ) -> LocalTxSubCommand<'a> {
        LocalTxSubCommand {
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
        let arg_tx_hash = Arg::with_name("tx-hash")
            .long("tx-hash")
            .takes_value(true)
            .validator(|input| FixedHashParser::<H256>::default().validate(input))
            .required(true)
            .help("Transaction hash");
        SubCommand::with_name("tx").subcommands(vec![
            SubCommand::with_name("add")
                .arg(
                    Arg::with_name("deps")
                        .long("deps")
                        .takes_value(true)
                        .validator(|input| CellOutPointParser.validate(input))
                        .multiple(true)
                        .help("Dependency cells"),
                )
                .arg(
                    Arg::with_name("inputs")
                        .long("inputs")
                        .takes_value(true)
                        .validator(|input| {
                            EitherParser::new(CellOutPointParser, NullParser).validate(input)
                        })
                        .multiple(true)
                        .help("Input cells"),
                )
                .arg(
                    Arg::with_name("outputs")
                        .long("outputs")
                        .takes_value(true)
                        .multiple(true)
                        .help("Output cells"),
                )
                .arg(
                    Arg::with_name("set-witnesses-by-keys")
                        .long("set-witnesses-by-keys")
                        .help("Set input witnesses by saved private keys"),
                ),
            SubCommand::with_name("set-witness")
                .arg(arg_tx_hash.clone())
                .arg(
                    Arg::with_name("input-index")
                        .long("input-index")
                        .takes_value(true)
                        .validator(|input| FromStrParser::<usize>::default().validate(input))
                        .required(true)
                        .help("Set witnesses for which input (index)"),
                )
                .arg(
                    Arg::with_name("witness")
                        .long("witness")
                        .takes_value(true)
                        .validator(|input| HexParser.validate(input))
                        .multiple(true)
                        .help("Witness data list"),
                ),
            SubCommand::with_name("set-witnesses-by-keys").arg(arg_tx_hash.clone()),
            SubCommand::with_name("show").arg(arg_tx_hash.clone()),
            SubCommand::with_name("remove").arg(arg_tx_hash.clone()),
            SubCommand::with_name("verify").arg(arg_tx_hash.clone()),
            SubCommand::with_name("list"),
        ])
    }
}

impl<'a> CliSubCommand for LocalTxSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        match matches.subcommand() {
            ("add", Some(m)) => {
                let deps: Vec<OutPoint> = CellOutPointParser
                    .from_matches_vec(m, "deps")?
                    .into_iter()
                    .map(|cell_out_point| OutPoint {
                        cell: cell_out_point,
                        block_hash: None,
                    })
                    .collect();
                let inputs: Vec<EitherValue<CellOutPoint, String>> =
                    EitherParser::new(CellOutPointParser, NullParser)
                        .from_matches_vec(m, "inputs")?;
                let inputs: Vec<CellInput> = inputs
                    .into_iter()
                    .map(|value| match value {
                        EitherValue::A(cell_out_point) => Ok(CellInput {
                            previous_output: OutPoint {
                                cell: Some(cell_out_point),
                                block_hash: None,
                            },
                            // TODO: Use a non-zero since
                            since: 0,
                        }),
                        EitherValue::B(input_name) => with_rocksdb(&self.db_path, None, |db| {
                            CellInputManager::new(db)
                                .get(&input_name)
                                .map_err(Into::into)
                        })
                        .map_err(|err| format!("{:?}", err)),
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let outputs_result: Result<Vec<CellOutput>, String> = m
                    .values_of_lossy("outputs")
                    .unwrap_or_else(Vec::new)
                    .into_iter()
                    .map(|output_name| {
                        let input = with_rocksdb(&self.db_path, None, |db| {
                            CellManager::new(db).get(&output_name).map_err(Into::into)
                        })
                        .map_err(|err| format!("{:?}", err))?;
                        Ok(input)
                    })
                    .collect();
                let outputs = outputs_result?;
                let set_witnesses_by_keys = m.is_present("set-witnesses-by-keys");

                let witnesses = inputs.iter().map(|_| Witness::new()).collect::<Vec<_>>();
                let mut tx = TransactionBuilder::default()
                    .deps(deps)
                    .inputs(inputs)
                    .outputs(outputs)
                    .witnesses(witnesses)
                    .build();
                with_rocksdb(&self.db_path, None, |db| {
                    TransactionManager::new(db).add(&tx).map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                if set_witnesses_by_keys {
                    let db_path = self.db_path.clone();
                    let secp_code_hash = self.genesis_info()?.secp_code_hash().clone();
                    tx = with_rocksdb(&db_path, None, |db| {
                        // TODO: use keystore
                        TransactionManager::new(db)
                            .set_witnesses_by_keys(
                                tx.hash(),
                                self.key_store,
                                self.rpc_client,
                                &secp_code_hash,
                            )
                            .map_err(Into::into)
                    })
                    .map_err(|err| format!("{:?}", err))?;
                }
                let tx_view: TransactionView = (&tx).into();
                Ok(tx_view.render(format, color))
            }
            ("set-witness", Some(m)) => {
                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let input_index: usize =
                    FromStrParser::<usize>::default().from_matches(m, "input-index")?;
                let witness: Vec<Bytes> = HexParser.from_matches_vec(m, "witness")?;
                let tx = with_rocksdb(&self.db_path, None, |db| {
                    TransactionManager::new(db)
                        .set_witness(&tx_hash, input_index, witness)
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let tx_view: TransactionView = (&tx).into();
                Ok(tx_view.render(format, color))
            }
            ("set-witnesses-by-keys", Some(m)) => {
                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let db_path = self.db_path.clone();
                let secp_code_hash = self.genesis_info()?.secp_code_hash().clone();
                let tx = with_rocksdb(&db_path, None, |db| {
                    // TODO: use keystore
                    TransactionManager::new(db)
                        .set_witnesses_by_keys(
                            &tx_hash,
                            self.key_store,
                            self.rpc_client,
                            &secp_code_hash,
                        )
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let tx_view: TransactionView = (&tx).into();
                Ok(tx_view.render(format, color))
            }
            ("show", Some(m)) => {
                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let tx = with_rocksdb(&self.db_path, None, |db| {
                    TransactionManager::new(db)
                        .get(&tx_hash)
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let tx_view: TransactionView = (&tx).into();
                Ok(tx_view.render(format, color))
            }
            ("remove", Some(m)) => {
                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let tx = with_rocksdb(&self.db_path, None, |db| {
                    TransactionManager::new(db)
                        .remove(&tx_hash)
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let tx_view: TransactionView = (&tx).into();
                Ok(tx_view.render(format, color))
            }
            ("verify", Some(m)) => {
                let tx_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "tx-hash")?;
                let db_path = self.db_path.clone();
                let result = with_rocksdb(&db_path, None, |db| {
                    TransactionManager::new(db)
                        .verify(&tx_hash, std::u64::MAX, self.rpc_client)
                        .map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                Ok(result.render(format, color))
            }
            ("list", Some(_m)) => {
                let txs = with_rocksdb(&self.db_path, None, |db| {
                    TransactionManager::new(db).list().map_err(Into::into)
                })
                .map_err(|err| format!("{:?}", err))?;
                let txs = txs
                    .into_iter()
                    .map(|tx| {
                        let tx_view: TransactionView = (&tx).into();
                        tx_view
                    })
                    .collect::<Vec<_>>();
                Ok(txs.render(format, color))
            }
            _ => Err(matches.usage().to_owned()),
        }
    }
}
