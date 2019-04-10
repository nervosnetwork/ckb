use super::super::setup::Setup;
use ckb_db::{CacheDB, RocksDB};
use ckb_instrument::{Export, Format};
use ckb_shared::shared::SharedBuilder;
use clap::{value_t, ArgMatches};

pub fn export(setup: &Setup, matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());
    let target = value_t!(matches.value_of("target"), String).unwrap_or_else(|e| e.exit());

    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(
            setup
                .chain_spec
                .to_consensus(&setup.configs.chain.spec)
                .unwrap(),
        )
        .db(&setup.configs.db)
        .build();
    Export::new(shared, format, target.into())
        .execute()
        .unwrap_or_else(|e| panic!("Export error {:?} ", e));
}
