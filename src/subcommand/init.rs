use crate::setup::{ExitCode, InitArgs};
use ckb_resource::{TemplateContext, AVAILABLE_SPECS};

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
    };

    if !args.force && args.locator.exported() {
        eprintln!("Config files already exists, use --force to overwrite.");
        return Err(ExitCode::Failure);
    }

    args.locator.export_ckb(&context)?;
    args.locator.export_miner(&context)?;

    if args.export_specs {
        args.locator.export_specs()?;
    }

    Ok(())
}
