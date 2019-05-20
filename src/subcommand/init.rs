use ckb_app_config::{ExitCode, InitArgs};
use ckb_resource::{
    TemplateContext, AVAILABLE_SPECS, CKB_CONFIG_FILE_NAME, DEFAULT_SPEC, MINER_CONFIG_FILE_NAME,
    SPEC_DEV_FILE_NAME,
};

pub fn init(args: InitArgs) -> Result<(), ExitCode> {
    if args.list_chains {
        for spec in AVAILABLE_SPECS {
            println!("{}", spec);
        }
        return Ok(());
    }

    let context = TemplateContext {
        spec: &args.chain,
        rpc_port: &args.rpc_port,
        p2p_port: &args.p2p_port,
        log_to_file: args.log_to_file,
        log_to_stdout: args.log_to_stdout,
    };

    let exported = args.locator.exported();
    if !args.force && exported {
        eprintln!("Config files already exists, use --force to overwrite.");
        return Err(ExitCode::Failure);
    }

    println!(
        "{} CKB directory in {}",
        if !exported {
            "Initialized"
        } else {
            "Reinitialized"
        },
        args.locator.root_dir().display()
    );

    println!("create {}", CKB_CONFIG_FILE_NAME);
    args.locator.export_ckb(&context)?;
    println!("create {}", MINER_CONFIG_FILE_NAME);
    args.locator.export_miner(&context)?;

    if args.chain == DEFAULT_SPEC {
        println!("create {}", SPEC_DEV_FILE_NAME);
        args.locator.export(SPEC_DEV_FILE_NAME, &context)?;
    }

    Ok(())
}
