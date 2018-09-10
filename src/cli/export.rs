use super::super::setup::Configs;
use ckb_instrument::{Export, Format};
use clap::ArgMatches;
use config_tool::{Config as ConfigTool, File, FileFormat};
use dir::default_base_path;
use {DEFAULT_CONFIG, DEFAULT_CONFIG_FILENAME};

pub fn export(matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());

    let data_path = matches
        .value_of("data-dir")
        .map(Into::into)
        .unwrap_or_else(default_base_path);

    let mut config_tool = ConfigTool::new();
    config_tool
        .merge(File::from_str(DEFAULT_CONFIG, FileFormat::Toml))
        .unwrap_or_else(|e| panic!("Config load error {:?} ", e));

    if let Some(config_path) = matches.value_of("config") {
        config_tool
            .merge(File::with_name(config_path).required(true))
            .unwrap_or_else(|e| panic!("Config load error {:?} ", e));
    } else {
        config_tool
            .merge(
                File::with_name(data_path.join(DEFAULT_CONFIG_FILENAME).to_str().unwrap())
                    .required(false),
            ).unwrap_or_else(|e| panic!("Config load error {:?} ", e));
    }

    let configs: Configs = config_tool
        .try_into()
        .unwrap_or_else(|e| panic!("Config load error {:?} ", e));

    let spec_type = matches
        .value_of("chain")
        .unwrap_or_else(|| &configs.ckb.chain);
    let target = value_t!(matches.value_of("target"), String).unwrap_or_else(|e| e.exit());
    Export::new(data_path, format, target.into(), spec_type)
        .and_then(|export| export.execute())
        .unwrap_or_else(|e| panic!("Export error {:?} ", e));
}
