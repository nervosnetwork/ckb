use ckb_app_config::{ExitCode, InitArgs};
use ckb_resource::{
    Resource, TemplateContext, AVAILABLE_SPECS, CKB_CONFIG_FILE_NAME, DEFAULT_SPEC,
    MINER_CONFIG_FILE_NAME, SPEC_DEV_FILE_NAME,
};
use ckb_script::Runner;

pub fn init(args: InitArgs) -> Result<(), ExitCode> {
    if args.list_chains {
        for spec in AVAILABLE_SPECS {
            println!("{}", spec);
        }
        return Ok(());
    }

    let runner = Runner::default().to_string();
    let context = TemplateContext {
        spec: &args.chain,
        rpc_port: &args.rpc_port,
        p2p_port: &args.p2p_port,
        log_to_file: args.log_to_file,
        log_to_stdout: args.log_to_stdout,
        runner: &runner,
    };

    let exported = Resource::exported_in(&args.root_dir);
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
        args.root_dir.display()
    );

    println!("create {}", CKB_CONFIG_FILE_NAME);
    Resource::bundled_ckb_config().export(&context, &args.root_dir)?;
    println!("create {}", MINER_CONFIG_FILE_NAME);
    Resource::bundled_miner_config().export(&context, &args.root_dir)?;

    if args.chain == DEFAULT_SPEC {
        println!("create {}", SPEC_DEV_FILE_NAME);
        Resource::bundled(SPEC_DEV_FILE_NAME.to_string()).export(&context, &args.root_dir)?;
    }

    Ok(())
}
