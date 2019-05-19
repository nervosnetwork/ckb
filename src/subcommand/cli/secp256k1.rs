use ckb_app_config::cli;
use ckb_app_config::ExitCode;
use clap::ArgMatches;
use crypto::secp::{Generator, Privkey, Pubkey};
use faster_hex::{hex_decode, hex_string};
use hash::blake2b_256;
use numext_fixed_hash::H160;
use std::fs;
use std::path::Path;

pub fn secp256k1<'m>(matches: &ArgMatches<'m>) -> Result<(), ExitCode> {
    let pubkey_vec = if matches.is_present(cli::ARG_GENERATE) {
        generate(matches)?
    } else {
        load(matches)?
    };

    let pubkey_hash = blake2b_256(&pubkey_vec);
    let pubkey_blake160 = H160::from_slice(&pubkey_hash[0..20]).unwrap();

    println!("[block_assembler]");
    println!("# secp256k1_sighash_all");
    println!("code_hash = \"0x9e3b3557f11b2b3532ce352bfe8017e9fd11d154c4c7f9b7aaaa1e621b539a08\"");
    println!("# args = [ \"blake160(compressed_pubkey)\" ]");
    println!("args = [ \"{:#x}\" ]", pubkey_blake160);

    Ok(())
}

pub fn generate<'m>(matches: &ArgMatches<'m>) -> Result<Vec<u8>, ExitCode> {
    let (privkey, pubkey) = Generator::new().random_keypair().unwrap();
    let pubkey_vec = pubkey.serialize();

    if let Some(path) = matches.value_of(cli::ARG_PRIVKEY) {
        if Path::new(path).exists() {
            eprintln!("Fail because privkey file already exists.");
            return Err(ExitCode::IO);
        }

        fs::write(path, format!("0x{}\n", privkey))?;
    }

    if let Some(path) = matches.value_of(cli::ARG_PUBKEY) {
        if Path::new(path).exists() {
            eprintln!("Fail because pubkey file already exists.");
            return Err(ExitCode::IO);
        }

        fs::write(path, format!("0x{}\n", hex_string(&pubkey_vec).unwrap()))?;
    }

    Ok(pubkey_vec)
}

pub fn load<'m>(matches: &ArgMatches<'m>) -> Result<Vec<u8>, ExitCode> {
    let pubkey_from_privkey: Option<Pubkey> = if let Some(path) = matches.value_of(cli::ARG_PRIVKEY)
    {
        let file_content = fs::read_to_string(path)?;
        let trimmed_content = file_content.trim();
        if trimmed_content.len() != 66 || &trimmed_content[..2] != "0x" {
            eprintln!("Privkey file corrupted: requires 32 bytes encoded in 0x prefix hex");
            return Err(ExitCode::IO);
        }

        Some(
            trimmed_content[2..]
                .parse::<Privkey>()
                .and_then(|key| key.pubkey())
                .map_err(|err| {
                    eprintln!("Privkey file corrupted: {}", err);
                    ExitCode::IO
                })?,
        )
    } else {
        None
    };

    let pubkey: Pubkey = if let Some(path) = matches.value_of(cli::ARG_PUBKEY) {
        let file_content = fs::read_to_string(path)?;
        let trimmed_content = file_content.trim();
        if trimmed_content.len() != 68 || &trimmed_content[..2] != "0x" {
            eprintln!("Pubkey file corrupted: requires 33 bytes encoded in 0x prefix hex");
            return Err(ExitCode::IO);
        }

        let mut pubkey_bytes = [0u8; 33];
        hex_decode(&trimmed_content.as_bytes()[2..], &mut pubkey_bytes).map_err(|err| {
            eprintln!("Pubkey file corrupted: {}", err);
            ExitCode::IO
        })?;

        let pubkey = Pubkey::from_slice(&pubkey_bytes).map_err(|err| {
            eprintln!("Pubkey file corrupted: {}", err);
            ExitCode::IO
        })?;

        if let Some(pubkey_from_privkey) = pubkey_from_privkey {
            if pubkey != pubkey_from_privkey {
                eprintln!("Pubkey and privkey do not match");
                return Err(ExitCode::Cli);
            }
        }

        pubkey
    } else {
        pubkey_from_privkey.unwrap()
    };

    Ok(pubkey.serialize())
}
