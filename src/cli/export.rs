use super::super::setup::Configs;
use ckb_chain_spec::SpecType;
use ckb_db::diskdb::RocksDB;
use ckb_instrument::{Export, Format};
use ckb_shared::cachedb::CacheDB;
use ckb_shared::shared::SharedBuilder;
use ckb_shared::store::ChainKVStore;
use clap::ArgMatches;
use config_tool::{Config as ConfigTool, File, FileFormat};
use dir::default_base_path;
use dir::Directories;
use std::path::Path;
use {DEFAULT_CONFIG, DEFAULT_CONFIG_FILENAME};

pub fn export(matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());
    let mut search_dirs = vec![];

    let data_path = matches
        .value_of("data-dir")
        .map(Into::into)
        .unwrap_or_else(default_base_path);
    search_dirs.push(data_path.clone());

    let mut config_tool = ConfigTool::new();
    config_tool
        .merge(File::from_str(DEFAULT_CONFIG, FileFormat::Toml))
        .unwrap_or_else(|e| panic!("Config load error {:?} ", e));

    if let Some(config_path) = matches.value_of("config") {
        config_tool
            .merge(File::with_name(config_path).required(true))
            .unwrap_or_else(|e| panic!("Config load error {:?} ", e));
        search_dirs.push(Path::new(config_path).parent().unwrap().to_path_buf());
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

    let dirs = Directories::new(&data_path);
    let db_path = dirs.join("db");

    let spec_type: SpecType = spec_type
        .parse()
        .unwrap_or_else(|e| panic!("parse spec error {:?} ", e));
    let spec = spec_type
        .load_spec::<String>(&search_dirs)
        .unwrap_or_else(|e| panic!("load spec error {:?} ", e));

    let shared = SharedBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
        .consensus(spec.to_consensus().unwrap())
        .build();
    Export::new(shared, format, target.into())
        .execute()
        .unwrap_or_else(|e| panic!("Export error {:?} ", e));
}
