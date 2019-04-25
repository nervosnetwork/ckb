use ckb_app_config::{ExitCode, InitArgs};
use ckb_resource::{
    TemplateContext, AVAILABLE_SPECS, CKB_CONFIG_FILE_NAME, MINER_CONFIG_FILE_NAME,
    SPECS_RESOURCE_DIR_NAME,
};

pub fn init(args: InitArgs) -> Result<(), ExitCode> {
    if args.list_specs {
        for spec in AVAILABLE_SPECS {
            println!("{}", spec);
        }
        return Ok(());
    }

    let context = TemplateContext {
        spec: &args.spec,
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

    println!("export {}", CKB_CONFIG_FILE_NAME);
    args.locator.export_ckb(&context)?;
    println!("export {}", MINER_CONFIG_FILE_NAME);
    args.locator.export_miner(&context)?;

    if args.export_specs {
        println!("export {}", SPECS_RESOURCE_DIR_NAME);
        args.locator.export_specs()?;
    }

    Ok(())
}
