//! this is a tool to generate rpc doc
mod gen;
use crate::gen::RpcDocGenerator;
use ckb_rpc::module::*;
use serde_json::json;
use std::fs;

const OPENRPC_DIR: &str = "./docs/ckb_rpc_openrpc/";

fn run_command(prog: &str, args: &[&str], dir: Option<&str>) -> Option<String> {
    std::process::Command::new(prog)
        .args(args)
        .current_dir(dir.unwrap_or("."))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|r| {
            String::from_utf8(r.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
}

fn get_tag() -> String {
    run_command("git", &["describe", "--tags", "--abbrev=0"], None).unwrap_or("main".to_owned())
}

/// Get git tag from command line
fn get_commit_sha() -> String {
    run_command("git", &["rev-parse", "HEAD"], Some(OPENRPC_DIR)).unwrap_or("main".to_string())
}

fn checkout_tag_branch(tag: &str) {
    let dir = Some(OPENRPC_DIR);
    let res = run_command("git", &["checkout", tag], dir);
    if res.is_none() {
        run_command("git", &["checkout", "-b", tag], dir);
    }
}

fn dump_openrpc_json() -> Result<(), Box<dyn std::error::Error>> {
    let dir = "./docs/ckb_rpc_openrpc/json/";
    let tag = get_tag();
    checkout_tag_branch(&tag);

    fs::create_dir_all(dir)?;
    let dump =
        |name: &str, doc: &mut serde_json::Value| -> Result<(), Box<dyn std::error::Error>> {
            doc["info"]["version"] = serde_json::Value::String(tag.clone());
            let obj = json!(doc);
            let res = serde_json::to_string_pretty(&obj)?;
            fs::write(dir.to_owned() + name, res)?;
            Ok(())
        };
    dump("alert_rpc_doc.json", &mut alert_rpc_doc())?;
    dump("net_rpc_doc.json", &mut net_rpc_doc())?;
    dump("subscription_rpc_doc.json", &mut subscription_rpc_doc())?;
    dump("debug_rpc_doc.json", &mut debug_rpc_doc())?;
    dump("chain_rpc_doc.json", &mut chain_rpc_doc())?;
    dump("miner_rpc_doc.json", &mut miner_rpc_doc())?;
    dump("pool_rpc_doc.json", &mut pool_rpc_doc())?;
    dump("stats_rpc_doc.json", &mut stats_rpc_doc())?;
    dump(
        "integration_test_rpc_doc.json",
        &mut integration_test_rpc_doc(),
    )?;
    dump("indexer_rpc_doc.json", &mut indexer_rpc_doc())?;
    dump("experiment_rpc_doc.json", &mut experiment_rpc_doc())?;
    eprintln!(
        "finished dump openrpc json for tag: {:?} at dir: {:?}",
        tag, dir
    );
    Ok(())
}

/// Generate rpc readme
pub fn gen_rpc_readme(readme_path: &str) -> Result<(), Box<dyn std::error::Error>> {
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

    let tag = get_commit_sha();
    let generator = RpcDocGenerator::new(&all_rpc, readme_path.to_owned(), tag);
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
