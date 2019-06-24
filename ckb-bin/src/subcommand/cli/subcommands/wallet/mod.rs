mod index;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use ckb_core::{block::Block, service::Request};
use ckb_crypto::secp::{Generator, SECP256K1};
use ckb_hash::blake2b_256;
use ckb_jsonrpc_types::BlockNumber;
use ckb_sdk::{Address, AddressFormat, NetworkType};
use clap::{App, Arg, ArgMatches, SubCommand};
use faster_hex::hex_string;
use numext_fixed_hash::{H160, H256};
use serde_json::json;

use super::CliSubCommand;
use crate::utils::arg_parser::{
    AddressParser, ArgParser, CapacityParser, FilePathParser, FixedHashParser, FromStrParser,
    HexParser, PrivkeyPathParser, PubkeyHexParser,
};
use crate::utils::printer::{OutputFormat, Printable};
use ckb_sdk::{
    build_witness_with_key, serialize_signature,
    wallet::{KeyStore, KeyStoreError},
    GenesisInfo, HttpRpcClient, IndexDatabase, TransferTransactionBuilder, LMDB_EXTRA_MAP_SIZE,
    MIN_SECP_CELL_CAPACITY, ONE_CKB,
};
pub use index::{
    start_index_thread, CapacityResult, IndexController, IndexRequest, IndexResponse,
    IndexThreadState, SimpleBlockInfo,
};

pub struct WalletSubCommand<'a> {
    rpc_client: &'a mut HttpRpcClient,
    key_store: &'a mut KeyStore,
    genesis_info: Option<GenesisInfo>,
    index_dir: PathBuf,
    index_controller: IndexController,
    interactive: bool,
}

impl<'a> WalletSubCommand<'a> {
    pub fn new(
        rpc_client: &'a mut HttpRpcClient,
        key_store: &'a mut KeyStore,
        genesis_info: Option<GenesisInfo>,
        index_dir: PathBuf,
        index_controller: IndexController,
        interactive: bool,
    ) -> WalletSubCommand<'a> {
        WalletSubCommand {
            rpc_client,
            key_store,
            genesis_info,
            index_dir,
            index_controller,
            interactive,
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

    fn get_db(&mut self) -> Result<IndexDatabase, String> {
        if !self.interactive {
            Request::call(self.index_controller.sender(), IndexRequest::Kick);
            for _ in 0..600 {
                let state = self.index_controller.state().read();
                if state.is_error() || state.is_stopped() {
                    break;
                } else if !state.is_synced() {
                    thread::sleep(Duration::from_millis(100));
                }
            }
            if !self.index_controller.state().read().is_synced() {
                return Err(format!(
                    "Index database not synced({}), please try again",
                    self.index_controller.state().read().to_string(),
                ));
            }
        }
        IndexDatabase::from_path(
            NetworkType::TestNet,
            self.genesis_info()?,
            self.index_dir.clone(),
            LMDB_EXTRA_MAP_SIZE,
        )
        .map_err(|err| err.to_string())
    }

    pub fn subcommand() -> App<'static, 'static> {
        let arg_privkey = Arg::with_name("privkey-path")
            .long("privkey-path")
            .takes_value(true)
            .validator(|input| PrivkeyPathParser.validate(input))
            .help("Private key file path (only read first line)");
        let arg_address = Arg::with_name("address")
            .long("address")
            .takes_value(true)
            .validator(|input| AddressParser.validate(input))
            .required(true)
            .help("Target address (see: https://github.com/nervosnetwork/ckb/wiki/Common-Address-Format)");
        let arg_lock_hash = Arg::with_name("lock-hash")
            .long("lock-hash")
            .takes_value(true)
            .validator(|input| FixedHashParser::<H256>::default().validate(input))
            .required(true)
            .help("Lock hash");
        SubCommand::with_name("wallet")
            .about("tranfer / query balance(with local index) / key utils")
            .subcommands(vec![
                SubCommand::with_name("transfer")
                    .about("Transfer capacity to an address (can have data)")
                    .arg(arg_privkey.clone())
                    .arg(
                        Arg::with_name("from-lock-arg")
                            .long("from-lock-arg")
                            .takes_value(true)
                            .validator(|input| FixedHashParser::<H160>::default().validate(input))
                            .help("The account's lock-arg (transfer from this account)")
                    )
                    .arg(
                        Arg::with_name("to-address")
                            .long("to-address")
                            .takes_value(true)
                            .validator(|input| AddressParser.validate(input))
                            .required(true)
                            .help("Target address"),
                    )
                    .arg(
                        Arg::with_name("to-data")
                            .long("to-data")
                            .takes_value(true)
                            .validator(|input| HexParser.validate(input))
                            .help("Hex data store in target cell (optional)"),
                    )
                    .arg(
                        Arg::with_name("capacity")
                            .long("capacity")
                            .takes_value(true)
                            .validator(|input| CapacityParser.validate(input))
                            .required(true)
                            .help("The capacity (unit: CKB, format: 123.335)"),
                    ),
                SubCommand::with_name("generate-key")
                    .about("Generate a random secp256k1 privkey and save to file (print block_assembler config)")
                    .arg(
                        Arg::with_name("print-privkey")
                            .long("print-privkey")
                            .help("Print privkey key (default: no)"),
                    )
                    .arg(
                        Arg::with_name("privkey-path")
                            .long("privkey-path")
                            .takes_value(true)
                            .validator(|input| FilePathParser::new(false).validate(input))
                            .required(true)
                            .help("Output privkey file path (content = privkey + address)"),
                    ),
                SubCommand::with_name("key-info")
                    .about("Show public information of a secp256k1 private key (from file) or public key")
                    .arg(arg_privkey.clone())
                    .arg(
                        Arg::with_name("pubkey")
                            .long("pubkey")
                            .takes_value(true)
                            .validator(|input| PubkeyHexParser.validate(input))
                            .required_if("privkey-path", "")
                            .help("Public key (hex string, compressed format)"),
                    ),
                SubCommand::with_name("get-capacity")
                    .about("Get capacity by lock script hash")
                    .arg(arg_lock_hash.clone()),
                SubCommand::with_name("get-live-cells")
                    .about("Get live cells by lock script hash")
                    .arg(arg_lock_hash.clone())
                    .arg(
                        Arg::with_name("limit")
                            .long("limit")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<usize>::default().validate(input))
                            .default_value("15")
                            .help("Get live cells <= limit")
                    )
                    .arg(
                        Arg::with_name("from")
                            .long("from")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u64>::default().validate(input))
                            .help("From block number"),
                    )
                    .arg(
                        Arg::with_name("to")
                            .long("to")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u64>::default().validate(input))
                            .help("To block number"),
                    ),
                SubCommand::with_name("get-lock-by-address")
                    .about("Get lock script (include hash) by address")
                    .arg(arg_address.clone()),
                SubCommand::with_name("get-balance")
                    .about("Get balance by address (balance is capacity)")
                    .arg(arg_address.clone()),
                SubCommand::with_name("top-capacity")
                    .about("Show top n capacity owned by lock script hash")
                    .arg(
                        Arg::with_name("number")
                            .short("n")
                            .long("number")
                            .takes_value(true)
                            .validator(|input| FromStrParser::<u32>::default().validate(input))
                            .default_value("10")
                            .help("Get top n capacity addresses"),
                    ),
                SubCommand::with_name("db-metrics")
                    .about("Show index database metrics"),
            ])
    }
}

impl<'a> CliSubCommand for WalletSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        match matches.subcommand() {
            ("transfer", Some(m)) => {
                let from_privkey: Option<secp256k1::SecretKey> =
                    PrivkeyPathParser.from_matches_opt(m, "privkey-path", false)?;
                let from_lock_arg: Option<H160> =
                    FixedHashParser::<H160>::default().from_matches(m, "from-lock-arg")?;
                let to_address: Address = AddressParser.from_matches(m, "to-address")?;
                let to_data: Bytes = HexParser
                    .from_matches_opt(m, "to-data", false)?
                    .unwrap_or_else(Bytes::new);
                let capacity: u64 = CapacityParser.from_matches(m, "capacity")?;

                if from_privkey.is_none() && from_lock_arg.is_none() {
                    return Err("<privkey-path> or <from-lock-arg> is required!".to_owned());
                }
                if capacity < MIN_SECP_CELL_CAPACITY {
                    return Err(format!(
                        "Capacity can not less than {} shannons",
                        MIN_SECP_CELL_CAPACITY
                    ));
                }
                if capacity < MIN_SECP_CELL_CAPACITY + (to_data.len() as u64 * ONE_CKB) {
                    return Err(format!(
                        "Capacity can not hold {} bytes of data",
                        to_data.len()
                    ));
                }

                let from_address = if let Some(from_privkey) = from_privkey {
                    let from_pubkey =
                        secp256k1::PublicKey::from_secret_key(&SECP256K1, &from_privkey);
                    let pubkey_hash = blake2b_256(&from_pubkey.serialize()[..]);
                    Address::from_lock_arg(&pubkey_hash[0..20])?
                } else {
                    Address::from_lock_arg(&from_lock_arg.as_ref().unwrap()[..])?
                };

                let genesis_info = self.genesis_info()?;
                let secp_code_hash = genesis_info.secp_code_hash();
                let mut total_capacity = 0;
                let infos = self.get_db()?.get_live_cell_infos(
                    from_address
                        .lock_script(secp_code_hash.clone())
                        .hash()
                        .clone(),
                    None,
                    |_, info| {
                        total_capacity += info.capacity;
                        let stop = total_capacity >= capacity;
                        (stop, true)
                    },
                );
                if total_capacity < capacity {
                    return Err(format!(
                        "Capacity not enough: {} => {}",
                        from_address.to_string(NetworkType::TestNet),
                        total_capacity,
                    ));
                }
                let tx_args = TransferTransactionBuilder {
                    from_address: &from_address,
                    from_capacity: total_capacity,
                    to_data: &to_data,
                    to_address: &to_address,
                    to_capacity: capacity,
                };
                let tx = if let Some(privkey) = from_privkey {
                    tx_args.build(infos, &genesis_info, |tx_hash| {
                        Ok(build_witness_with_key(&privkey, tx_hash))
                    })?
                } else {
                    let lock_arg = from_lock_arg.as_ref().unwrap();
                    tx_args.build(infos, &genesis_info, |tx_hash| {
                        self.key_store
                            .sign_recoverable(lock_arg, tx_hash)
                            .map(|signature| serialize_signature(&signature))
                            .map_err(|err| {
                                match err {
                                    KeyStoreError::AccountLocked(lock_arg) => {
                                        format!("Account(lock_arg={:x}) locked or not exists, your may use `account unlock` to unlock it", lock_arg)
                                    }
                                    err => err.to_string(),
                                }
                            })
                    })?
                };
                println!("[Send Transaction]:\n{}", tx.render(format, color),);
                let resp = self
                    .rpc_client
                    .send_transaction(tx)
                    .call()
                    .map_err(|err| format!("Send transaction error: {}", err))?;
                Ok(resp.render(format, color))
            }
            ("generate-key", Some(m)) => {
                let (privkey, pubkey) = Generator::new()
                    .random_keypair()
                    .expect("generate random key error");
                let print_privkey = m.is_present("print-privkey");
                let privkey_path = m.value_of("privkey-path").unwrap();
                let pubkey_string = hex_string(&pubkey.serialize()).expect("encode pubkey failed");
                let address = Address::from_pubkey(AddressFormat::default(), &pubkey)?;
                let address_string = address.to_string(NetworkType::TestNet);

                if Path::new(privkey_path).exists() {
                    return Err(format!(
                        "ERROR: output path ( {} ) already exists",
                        privkey_path
                    ));
                }
                let mut file = fs::File::create(privkey_path).map_err(|err| err.to_string())?;
                file.write(format!("{}\n", privkey.to_string()).as_bytes())
                    .map_err(|err| err.to_string())?;
                file.write(format!("{}\n", address_string).as_bytes())
                    .map_err(|err| err.to_string())?;

                let genesis_info = self.genesis_info()?;
                let secp_code_hash = genesis_info.secp_code_hash();
                println!(
                    r#"Put this config in < ckb.toml >:

[block_assembler]
code_hash = "{:#x}"
args = ["{:#x}"]
"#,
                    secp_code_hash,
                    address.hash()
                );

                let mut resp = json!({
                    "pubkey": pubkey_string,
                    "address": address_string,
                    "lock_hash": address.lock_script(secp_code_hash.clone()).hash(),
                });
                if print_privkey {
                    resp.as_object_mut()
                        .unwrap()
                        .insert("privkey".to_owned(), privkey.to_string().into());
                }
                Ok(resp.render(format, color))
            }
            ("key-info", Some(m)) => {
                let secp_key_opt: Option<secp256k1::SecretKey> =
                    PrivkeyPathParser.from_matches_opt(m, "privkey-path", false)?;
                let pubkey = secp_key_opt
                    .map(|privkey| {
                        Result::<_, String>::Ok(secp256k1::PublicKey::from_secret_key(
                            &SECP256K1, &privkey,
                        ))
                    })
                    .unwrap_or_else(|| {
                        let key: secp256k1::PublicKey =
                            PubkeyHexParser.from_matches(m, "pubkey")?;
                        Ok(key)
                    })?;
                let pubkey_string =
                    hex_string(&pubkey.serialize()[..]).expect("encode pubkey failed");
                let address = {
                    let pubkey_hash = blake2b_256(&pubkey.serialize()[..]);
                    Address::from_lock_arg(&pubkey_hash[0..20])?
                };
                let address_string = address.to_string(NetworkType::TestNet);

                let genesis_info = self.genesis_info()?;
                let secp_code_hash = genesis_info.secp_code_hash();
                println!(
                    r#"Put this config in < ckb.toml >:

[block_assembler]
code_hash = "{:#x}"
args = ["{:#x}"]
"#,
                    secp_code_hash,
                    address.hash()
                );

                let resp = json!({
                    "pubkey": pubkey_string,
                    "address": address_string,
                    "lock_hash": address.lock_script(secp_code_hash.clone()).hash(),
                });
                Ok(resp.render(format, color))
            }
            ("get-capacity", Some(m)) => {
                let lock_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "lock-hash")?;
                let resp = serde_json::json!({
                    "capacity": self.get_db()?.get_capacity(lock_hash)
                });
                Ok(resp.render(format, color))
            }
            ("get-live-cells", Some(m)) => {
                let lock_hash: H256 =
                    FixedHashParser::<H256>::default().from_matches(m, "lock-hash")?;
                let limit: usize = FromStrParser::<usize>::default().from_matches(m, "limit")?;
                let from_number_opt: Option<u64> =
                    FromStrParser::<u64>::default().from_matches_opt(m, "from", false)?;
                let to_number_opt: Option<u64> =
                    FromStrParser::<u64>::default().from_matches_opt(m, "to", false)?;

                let to_number = to_number_opt.unwrap_or(std::u64::MAX);
                let mut total_capacity = 0;
                let infos = self.get_db()?.get_live_cell_infos(
                    lock_hash.clone(),
                    from_number_opt,
                    |idx, info| {
                        let stop = idx >= limit || info.number > to_number;
                        let push_info = !stop;
                        if push_info {
                            total_capacity += info.capacity;
                        }
                        (stop, push_info)
                    },
                );
                let resp = serde_json::json!({
                    "live_cells": infos.into_iter().map(|info| {
                        serde_json::to_value(&info).unwrap()
                    }).collect::<Vec<_>>(),
                    "total_capacity": total_capacity,
                });
                Ok(resp.render(format, color))
            }
            ("get-lock-by-address", Some(m)) => {
                let address: Address = AddressParser.from_matches(m, "address")?;
                let db = self.get_db()?;
                let lock_script = db
                    .get_lock_hash_by_address(address)
                    .and_then(|lock_hash| db.get_lock_script_by_hash(lock_hash))
                    .map(|lock_script| {
                        let args = lock_script
                            .args
                            .iter()
                            .map(|arg| hex_string(arg).unwrap())
                            .collect::<Vec<_>>();
                        serde_json::json!({
                            "hash": lock_script.hash(),
                            "script": {
                                "code_hash": lock_script.code_hash,
                                "args": args,
                            }
                        })
                    });
                Ok(lock_script.render(format, color))
            }
            ("get-balance", Some(m)) => {
                let address: Address = AddressParser.from_matches(m, "address")?;
                let secp_code_hash = self.genesis_info()?.secp_code_hash().clone();
                let lock_hash = address.lock_script(secp_code_hash).hash().clone();
                let resp = serde_json::json!({
                    "capacity": self.get_db()?.get_capacity(lock_hash)
                });
                Ok(resp.render(format, color))
            }
            ("top-capacity", Some(m)) => {
                let n: usize = m
                    .value_of("number")
                    .map(|n_str| n_str.parse().unwrap())
                    .unwrap();
                let resp = serde_json::to_value({
                    self.get_db()?
                        .get_top_n(n)
                        .into_iter()
                        .map(|(lock_hash, address, capacity)| {
                            serde_json::json!({
                                "lock_hash": format!("{:#x}", lock_hash),
                                "address": address.map(|addr| addr.to_string(NetworkType::TestNet)),
                                "capacity": capacity,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            ("db-metrics", _) => {
                let resp = serde_json::to_value(self.get_db()?.get_metrics(None))
                    .map_err(|err| err.to_string())?;
                Ok(resp.render(format, color))
            }
            _ => Err(matches.usage().to_owned()),
        }
    }
}
