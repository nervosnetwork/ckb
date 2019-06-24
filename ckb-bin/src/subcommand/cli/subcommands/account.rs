use std::path::PathBuf;
use std::time::Duration;

use ckb_sdk::{wallet::KeyStore, Address, NetworkType};
use clap::{App, Arg, ArgMatches, SubCommand};
use numext_fixed_hash::H160;

use super::CliSubCommand;
use crate::utils::{
    arg_parser::{ArgParser, DurationParser, FixedHashParser, PrivkeyPathParser},
    other::read_password,
    printer::{OutputFormat, Printable},
};

pub struct AccountSubCommand<'a> {
    key_store: &'a mut KeyStore,
}

impl<'a> AccountSubCommand<'a> {
    pub fn new(key_store: &'a mut KeyStore) -> AccountSubCommand<'a> {
        AccountSubCommand { key_store }
    }

    pub fn subcommand(name: &'static str) -> App<'static, 'static> {
        SubCommand::with_name(name)
            .about("Management accounts")
            .subcommands(vec![
                SubCommand::with_name("list"),
                SubCommand::with_name("new"),
                SubCommand::with_name("import")
                    .about("Imports an unencrypted private key from <keyfile> and creates a new account.")
                    .arg(
                        Arg::with_name("keyfile")
                            .long("keyfile")
                            .takes_value(true)
                            .validator(|input| PrivkeyPathParser.validate(input))
                            .required(true)
                            .help("The keyfile is assumed to contain an unencrypted private key in hexadecimal format.")
                    ) ,
                SubCommand::with_name("unlock")
                    .about("Unlock an account")
                    .arg(
                        Arg::with_name("lock-arg")
                            .long("lock-arg")
                            .takes_value(true)
                            .validator(|input| FixedHashParser::<H160>::default().validate(input))
                            .required(true)
                            .help("The lock_arg (identifier) of the account")
                    )
                    .arg(
                        Arg::with_name("keep")
                            .long("keep")
                            .takes_value(true)
                            .validator(|input| DurationParser.validate(input))
                            .default_value("30m")
                            .help("How long before the key expired (repeat unlock will increase the time)")
                    ),
                SubCommand::with_name("update"),
                SubCommand::with_name("export"),
            ])
    }
}

impl<'a> CliSubCommand for AccountSubCommand<'a> {
    fn process(
        &mut self,
        matches: &ArgMatches,
        format: OutputFormat,
        color: bool,
    ) -> Result<String, String> {
        match matches.subcommand() {
            ("list", _) => {
                let mut accounts = self
                    .key_store
                    .get_accounts()
                    .iter()
                    .map(|(address, filepath)| (address.clone(), filepath.clone()))
                    .collect::<Vec<(H160, PathBuf)>>();
                accounts.sort_by(|a, b| a.1.cmp(&b.1));
                let resp = accounts
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (lock_arg, filepath))| {
                        let address = Address::from_lock_arg(&lock_arg[..]).unwrap();
                        let timeout = self.key_store.get_lock_timeout(&lock_arg);
                        let status = timeout
                            .map(|timeout| format!("lock after: {}", timeout))
                            .unwrap_or_else(|| "locked".to_owned());
                        serde_json::json!({
                            "#": idx,
                            "lock_arg": format!("{:x}", lock_arg),
                            "address": {
                                "mainnet": address.to_string(NetworkType::MainNet),
                                "testnet": address.to_string(NetworkType::TestNet),
                            },
                            "path": filepath.to_string_lossy(),
                            "status": status,
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(serde_json::json!(resp).render(format, color))
            }
            ("new", _) => {
                println!("Your new account is locked with a password. Please give a password. Do not forget this password.");

                let pass = read_password(true)?;
                let lock_arg = self
                    .key_store
                    .new_account(pass.as_bytes())
                    .map_err(|err| err.to_string())?;
                let address = Address::from_lock_arg(&lock_arg[..]).unwrap();
                let resp = serde_json::json!({
                    "lock_arg": format!("{:x}", lock_arg),
                    "address": {
                        "mainnet": address.to_string(NetworkType::MainNet),
                        "testnet": address.to_string(NetworkType::TestNet),
                    },
                });
                Ok(resp.render(format, color))
            }
            ("import", Some(m)) => {
                let secp_key: secp256k1::SecretKey =
                    PrivkeyPathParser.from_matches(m, "keyfile")?;
                let password = read_password(true)?;
                let lock_arg = self
                    .key_store
                    .import_secp_key(&secp_key, password.as_bytes())
                    .map_err(|err| err.to_string())?;
                let address = Address::from_lock_arg(&lock_arg[..]).unwrap();
                let resp = serde_json::json!({
                    "lock_arg": format!("{:x}", lock_arg),
                    "address": {
                        "mainnet": address.to_string(NetworkType::MainNet),
                        "testnet": address.to_string(NetworkType::TestNet),
                    },
                });
                Ok(resp.render(format, color))
            }
            ("unlock", Some(m)) => {
                let lock_arg: H160 =
                    FixedHashParser::<H160>::default().from_matches(m, "lock-arg")?;
                let keep: Duration = DurationParser.from_matches(m, "keep")?;
                let password = read_password(false)?;
                let lock_after = self
                    .key_store
                    .timed_unlock(&lock_arg, password.as_bytes(), keep)
                    .map(|timeout| timeout.to_string())
                    .map_err(|err| err.to_string())?;
                let resp = serde_json::json!({
                    "lock-after": lock_after,
                });
                Ok(resp.render(format, color))
            }
            // ("update", Some(m)) => {
            // }
            // ("export", Some(m)) => {
            // }
            _ => Err(matches.usage().to_owned()),
        }
    }
}
