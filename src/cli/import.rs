use super::super::setup::Setup;
use ckb_chain::chain::ChainBuilder;
use ckb_db::RocksDB;
use ckb_instrument::{Format, Import};
use ckb_notify::NotifyService;
use ckb_shared::cachedb::CacheDB;
use ckb_shared::shared::SharedBuilder;
use clap::{value_t, ArgMatches};

pub fn import(setup: &Setup, matches: &ArgMatches) {
    let format = value_t!(matches.value_of("format"), Format).unwrap_or_else(|e| e.exit());
    let source = value_t!(matches.value_of("source"), String).unwrap_or_else(|e| e.exit());

    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(
            setup
                .chain_spec
                .to_consensus(&setup.configs.chain.spec)
                .unwrap(),
        )
        .db(&setup.configs.db)
        .build();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainBuilder::new(shared.clone(), notify).build();
    let chain_controller = chain_service.start::<&str>(Some("ImportChainService"));

    Import::new(chain_controller, format, source.into())
        .execute()
        .unwrap_or_else(|e| panic!("Import error {:?} ", e));
}
