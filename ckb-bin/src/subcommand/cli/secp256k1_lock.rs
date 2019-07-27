use super::parse_hex_data;
use ckb_app_config::{cli, ExitCode};
use ckb_crypto::secp::Pubkey;
use ckb_hash::blake2b_256;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use clap::ArgMatches;
use numext_fixed_hash::H160;

pub fn secp256k1_lock<'m>(matches: &ArgMatches<'m>) -> Result<(), ExitCode> {
    let pubkey_bytes = parse_hex_data(matches.value_of(cli::ARG_DATA).unwrap())?;
    if pubkey_bytes.len() != 65 && pubkey_bytes.len() != 33 {
        eprintln!(
            "Expect pubkey length 65 (uncompressed) or 33 (compressed), actual: {}",
            pubkey_bytes.len()
        );
        return Err(ExitCode::IO);
    }

    let pubkey = Pubkey::from_slice(&pubkey_bytes).map_err(|err| {
        eprintln!("Pubkey corrupted: {}", err);
        ExitCode::IO
    })?;

    let pubkey_hash = blake2b_256(&pubkey.serialize());
    let pubkey_blake160 = H160::from_slice(&pubkey_hash[0..20]).unwrap();

    match matches.value_of(cli::ARG_FORMAT).unwrap() {
        "toml" => {
            println!("[block_assembler]");
            println!("# secp256k1_blake160_sighash_all");
            println!(
                "code_hash = \"{:#x}\"",
                CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL
            );
            println!("# args = [ \"ckb cli blake160 <compressed-pubkey>\" ]");
            println!("args = [ \"{:#x}\" ]", pubkey_blake160);
        }
        "cmd" => {
            println!(
                "--ba-code-hash {:#x} --ba-arg {:#x}",
                CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL, pubkey_blake160
            );
        }
        _ => unreachable!(),
    }

    Ok(())
}
