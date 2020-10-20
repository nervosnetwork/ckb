use std::fs;
use std::io::{self, Read};

use crate::helper::prompt;
use ckb_app_config::{ExitCode, InitArgs};
use ckb_chain_spec::ChainSpec;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_resource::{
    Resource, TemplateContext, AVAILABLE_SPECS, CKB_CONFIG_FILE_NAME, DB_OPTIONS_FILE_NAME,
    MINER_CONFIG_FILE_NAME, SPEC_DEV_FILE_NAME,
};
use ckb_types::{prelude::*, H256};

const DEFAULT_LOCK_SCRIPT_HASH_TYPE: &str = "type";
const SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN: usize = 20 * 2 + 2; // 42 = 20 x 2 + prefix 0x

pub fn init(args: InitArgs) -> Result<(), ExitCode> {
    let mut args = args;

    if args.list_chains {
        for spec in AVAILABLE_SPECS {
            println!("{}", spec);
        }
        return Ok(());
    }

    let exported = Resource::exported_in(&args.root_dir);
    if !args.force && exported {
        eprintln!("Config files already exist, use --force to overwrite.");

        if args.interactive {
            let input = prompt("Overwrite config files now? ");

            if !["y", "Y"].contains(&input.trim()) {
                return Err(ExitCode::Failure);
            }
        } else {
            return Err(ExitCode::Failure);
        }
    }

    if args.interactive {
        let in_block_assembler_code_hash = prompt("code hash: ");
        let in_args = prompt("args: ");
        let in_hash_type = prompt("hash_type: ");
        let in_message = prompt("message: ");

        args.block_assembler_code_hash = Some(in_block_assembler_code_hash.trim().to_string());

        args.block_assembler_args = in_args
            .trim()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();

        args.block_assembler_message = Some(in_message.trim().to_string());

        match serde_plain::from_str::<ScriptHashType>(in_hash_type.trim()).ok() {
            Some(hash_type) => args.block_assembler_hash_type = hash_type,
            None => eprintln!("Invalid block assembler hash type"),
        }
    }

    // Try to find the default secp256k1 from bundled chain spec.
    let default_code_hash_option =
        ChainSpec::load_from(&Resource::bundled(format!("specs/{}.toml", args.chain)))
            .ok()
            .map(|spec| {
                let hash: H256 = spec
                    .build_consensus()
                    .expect("Build consensus failed")
                    .get_secp_type_script_hash()
                    .unpack();
                format!("{:#x}", hash)
            });

    let block_assembler_code_hash = args.block_assembler_code_hash.as_ref().or_else(|| {
        if !args.block_assembler_args.is_empty() {
            default_code_hash_option.as_ref()
        } else {
            None
        }
    });

    let block_assembler = match block_assembler_code_hash {
        Some(hash) => {
            if let Some(default_code_hash) = &default_code_hash_option {
                if ScriptHashType::Type != args.block_assembler_hash_type {
                    eprintln!(
                        "WARN: the default lock should use hash type `{}`, you are using `{}`.\n\
                         It will require `ckb run --ba-advanced` to enable this block assembler",
                        DEFAULT_LOCK_SCRIPT_HASH_TYPE, args.block_assembler_hash_type
                    );
                } else if *default_code_hash != *hash {
                    eprintln!(
                        "WARN: the default secp256k1 code hash is `{}`, you are using `{}`.\n\
                         It will require `ckb run --ba-advanced` to enable this block assembler",
                        default_code_hash, hash
                    );
                } else if args.block_assembler_args.len() != 1
                    || args.block_assembler_args[0].len() != SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN
                {
                    eprintln!(
                        "WARN: the block assembler arg is not a valid secp256k1 pubkey hash.\n\
                         It will require `ckb run --ba-advanced` to enable this block assembler"
                    );
                }
            }
            format!(
                "[block_assembler]\n\
                 code_hash = \"{}\"\n\
                 args = \"{}\"\n\
                 hash_type = \"{}\"\n\
                 message = \"{}\"",
                hash,
                args.block_assembler_args.join("\", \""),
                serde_plain::to_string(&args.block_assembler_hash_type).unwrap(),
                args.block_assembler_message
                    .unwrap_or_else(|| "0x".to_string()),
            )
        }
        None => {
            eprintln!("WARN: mining feature is disabled because of lacking the block assembler config options");
            format!(
                "# secp256k1_blake160_sighash_all example:\n\
                 # [block_assembler]\n\
                 # code_hash = \"{}\"\n\
                 # args = \"ckb cli blake160 <compressed-pubkey>\"\n\
                 # hash_type = \"{}\"\n\
                 # message = \"A 0x-prefixed hex string\"",
                default_code_hash_option.unwrap_or_default(),
                DEFAULT_LOCK_SCRIPT_HASH_TYPE,
            )
        }
    };

    println!(
        "{} CKB directory in {}",
        if !exported {
            "Initialized"
        } else {
            "Reinitialized"
        },
        args.root_dir.display()
    );

    let log_to_file = args.log_to_file.to_string();
    let log_to_stdout = args.log_to_stdout.to_string();
    let mut context = TemplateContext::new(
        &args.chain,
        vec![
            ("rpc_port", args.rpc_port.as_str()),
            ("p2p_port", args.p2p_port.as_str()),
            ("log_to_file", log_to_file.as_str()),
            ("log_to_stdout", log_to_stdout.as_str()),
            ("block_assembler", block_assembler.as_str()),
            ("spec_source", "bundled"),
        ],
    );

    if let Some(spec_file) = args.import_spec {
        context.insert("spec_source", "file");

        let specs_dir = args.root_dir.join("specs");
        fs::create_dir_all(&specs_dir)?;
        let target_file = specs_dir.join(format!("{}.toml", args.chain));

        if spec_file == "-" {
            println!("create specs/{}.toml from stdin", args.chain);
            let mut encoded_content = String::new();
            io::stdin().read_to_string(&mut encoded_content)?;
            let spec_content = base64::decode_config(
                &encoded_content.trim(),
                base64::STANDARD.decode_allow_trailing_bits(true),
            )
            .map_err(|err| {
                eprintln!("stdin must be encoded in base64: {}", err);
                ExitCode::Failure
            })?;
            fs::write(target_file, spec_content)?;
        } else {
            println!("cp {} specs/{}.toml", spec_file, args.chain);
            fs::copy(spec_file, target_file)?;
        }
    } else if args.chain == "dev" {
        println!("create {}", SPEC_DEV_FILE_NAME);
        Resource::bundled(SPEC_DEV_FILE_NAME.to_string()).export(&context, &args.root_dir)?;
    }

    println!("create {}", CKB_CONFIG_FILE_NAME);
    Resource::bundled_ckb_config().export(&context, &args.root_dir)?;
    println!("create {}", MINER_CONFIG_FILE_NAME);
    Resource::bundled_miner_config().export(&context, &args.root_dir)?;
    println!("create {}", DB_OPTIONS_FILE_NAME);
    Resource::bundled_db_options().export(&context, &args.root_dir)?;

    Ok(())
}
