//! this is a tool to generate rpc doc
use ckb_rpc::module::*;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = "./target/doc/ckb_rpc_openrpc/";
    fs::create_dir_all(dir)?;
    fs::write(
        dir.to_owned() + "alert_rpc_doc.json",
        alert_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "net_rpc_doc.json",
        net_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "subscription_rpc_doc.json",
        subscription_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "debug_rpc_doc.json",
        debug_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "chain_rpc_doc.json",
        chain_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "miner_rpc_doc.json",
        miner_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "pool_rpc_doc.json",
        pool_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "stats_rpc_doc.json",
        stats_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "integration_test_rpc_doc.json",
        integration_test_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "indexer_rpc_doc.json",
        indexer_rpc_doc().to_string(),
    )?;
    fs::write(
        dir.to_owned() + "experiment_rpc_doc.json",
        experiment_rpc_doc().to_string(),
    )?;
    Ok(())
}
