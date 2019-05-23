use super::parse_hex_data;
use ckb_app_config::{cli, ExitCode};
use clap::ArgMatches;
use crypto::secp::Pubkey;
use hash::blake2b_256;
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
        "block_assembler" => {
            println!("[block_assembler]");
            println!("# secp256k1_sighash_all");
            println!("code_hash = \"0x9e3b3557f11b2b3532ce352bfe8017e9fd11d154c4c7f9b7aaaa1e621b539a08\"");
            println!("# args = [ \"blake160(compressed_pubkey)\" ]");
            println!("args = [ \"{:#x}\" ]", pubkey_blake160);
        }
        "json" => {
            println!("{{");
            println!("    \"code_hash\": \"0x9e3b3557f11b2b3532ce352bfe8017e9fd11d154c4c7f9b7aaaa1e621b539a08\",");
            println!("    \"args\": [");
            println!("        \"{:#x}\"", pubkey_blake160);
            println!("    ]");
            println!("}}");
        }
        _ => unreachable!(),
    }

    Ok(())
}
