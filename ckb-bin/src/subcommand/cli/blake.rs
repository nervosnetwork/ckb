use super::parse_hex_data;
use ckb_app_config::{cli, ExitCode};
use ckb_hash::blake2b_256;
use clap::ArgMatches;
use numext_fixed_hash::{H160, H256};

pub fn blake256<'m>(matches: &ArgMatches<'m>) -> Result<(), ExitCode> {
    let data = parse_hex_data(matches.value_of(cli::ARG_DATA).unwrap())?;
    let result = blake2b_256(data.as_slice());
    println!("{:#x}", H256::from_slice(&result).expect("H256"));
    Ok(())
}

pub fn blake160<'m>(matches: &ArgMatches<'m>) -> Result<(), ExitCode> {
    let data = parse_hex_data(matches.value_of(cli::ARG_DATA).unwrap())?;
    let result = blake2b_256(data.as_slice());
    println!("{:#x}", H160::from_slice(&result[..20]).expect("H160"));
    Ok(())
}
