//! this is a tool to generate rpc doc
mod gen;
use crate::gen::RpcDocGenerator;
use ckb_rpc::module::*;
use std::fs;

fn dump_openrpc_json() -> Result<(), Box<dyn std::error::Error>> {
    let dir = "./target/doc/ckb_rpc_openrpc/";
    fs::create_dir_all(dir)?;
    let dump = |name: &str, doc: serde_json::Value| -> Result<(), Box<dyn std::error::Error>> {
        fs::write(dir.to_owned() + name, doc.to_string())?;
        Ok(())
    };
    dump("alert_rpc_doc.json", alert_rpc_doc())?;
    dump("net_rpc_doc.json", net_rpc_doc())?;
    dump("subscription_rpc_doc.json", subscription_rpc_doc())?;
    dump("debug_rpc_doc.json", debug_rpc_doc())?;
    dump("chain_rpc_doc.json", chain_rpc_doc())?;
    dump("miner_rpc_doc.json", miner_rpc_doc())?;
    dump("pool_rpc_doc.json", pool_rpc_doc())?;
    dump("stats_rpc_doc.json", stats_rpc_doc())?;
    dump("integration_test_rpc_doc.json", integration_test_rpc_doc())?;
    dump("indexer_rpc_doc.json", indexer_rpc_doc())?;
    dump("experiment_rpc_doc.json", experiment_rpc_doc())?;
    eprintln!("finished dump openrpc json...");
    Ok(())
}

fn gen_rpc_readme(readme_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let all_rpc = vec![
        alert_rpc_doc(),
        net_rpc_doc(),
        subscription_rpc_doc(),
        debug_rpc_doc(),
        chain_rpc_doc(),
        miner_rpc_doc(),
        pool_rpc_doc(),
        stats_rpc_doc(),
        integration_test_rpc_doc(),
        indexer_rpc_doc(),
        experiment_rpc_doc(),
    ];

    let generator = RpcDocGenerator::new(&all_rpc, readme_path.to_owned());
    fs::write(readme_path, generator.gen_markdown())?;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if args[1] == "--json" {
            return dump_openrpc_json();
        }
        gen_rpc_readme(&args[1])?;
    }
    Ok(())
}
