use super::super::setup::Setup;
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_db::diskdb::RocksDB;
use ckb_instrument::{Format, Import};
use ckb_shared::cachedb::CacheDB;
use ckb_shared::shared::SharedBuilder;
use clap::{value_t, ArgMatches};

pub fn import(setup: &Setup, matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());
    let source = value_t!(matches.value_of("source"), String).unwrap_or_else(|e| e.exit());

    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(setup.chain_spec.to_consensus().unwrap())
        .db(&setup.configs.db)
        .build();
    let (chain_controller, chain_receivers) = ChainController::build();
    let chain_service = ChainBuilder::new(shared).build();
    let _handle = chain_service.start(Some("ImportChainService"), chain_receivers);

    Import::new(chain_controller, format, source.into())
        .execute()
        .unwrap_or_else(|e| panic!("Import error {:?} ", e));
}
