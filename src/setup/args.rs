use super::app_config::CKBAppConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_instrument::Format;
use ckb_miner::MinerConfig;
use ckb_pow::PowEngine;
use ckb_resource::ResourceLocator;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ExportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub format: Format,
    pub target: PathBuf,
}

pub struct ImportArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
    pub format: Format,
    pub source: PathBuf,
}

pub struct RunArgs {
    pub config: Box<CKBAppConfig>,
    pub consensus: Consensus,
}

pub struct MinerArgs {
    pub config: MinerConfig,
    pub pow_engine: Arc<dyn PowEngine>,
}

pub struct InitArgs {
    pub locator: ResourceLocator,
    pub spec: String,
    pub rpc_port: String,
    pub p2p_port: String,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
    pub export_specs: bool,
    pub list_specs: bool,
    pub force: bool,
}
