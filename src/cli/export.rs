use super::super::setup::Setup;
use ckb_db::diskdb::RocksDB;
use ckb_instrument::{Export, Format};
use ckb_shared::cachedb::CacheDB;
use ckb_shared::shared::SharedBuilder;
use ckb_shared::store::ChainKVStore;
use clap::{value_t, ArgMatches};

pub fn export(setup: &Setup, matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());
    let target = value_t!(matches.value_of("target"), String).unwrap_or_else(|e| e.exit());

    let db_path = setup.dirs.join("db");

    let shared = SharedBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
        .consensus(setup.chain_spec.to_consensus().unwrap())
        .build();
    Export::new(shared, format, target.into())
        .execute()
        .unwrap_or_else(|e| panic!("Export error {:?} ", e));
}
