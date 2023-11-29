//! this is a tool to generate rpc doc
mod gen;
use crate::gen::RpcDocGenerator;
use ckb_rpc::module::*;
use serde_json::json;
use std::{fs, path::PathBuf};

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

fn get_version() -> String {
    let version = run_command("cargo", &["pkgid"], None)
        .unwrap()
        .split('#')
        .nth(1)
        .unwrap_or("0.0.0")
        .to_owned();
    eprintln!("version: {:?}", version);
    return version;
}

fn get_commit_sha() -> String {
    let res =
        run_command("git", &["rev-parse", "HEAD"], Some(OPENRPC_DIR)).unwrap_or("main".to_string());
    eprintln!("commit sha: {:?}", res);
    res
}

fn checkout_tag_branch(version: &str) {
    let dir = Some(OPENRPC_DIR);
    let res = run_command("git", &["checkout", version], dir);
    if res.is_none() {
        run_command("git", &["checkout", "-b", version], dir);
    }
}

fn dump_openrpc_json() -> Result<(), Box<dyn std::error::Error>> {
    let json_dir = PathBuf::from(OPENRPC_DIR).join("json");
    let version: String = get_version();
    checkout_tag_branch(&version);
    fs::create_dir_all(&json_dir)?;
    let dump =
        |name: &str, doc: &mut serde_json::Value| -> Result<(), Box<dyn std::error::Error>> {
            doc["info"]["version"] = serde_json::Value::String(version.clone());
            let obj = json!(doc);
            let res = serde_json::to_string_pretty(&obj)?;
            fs::write(json_dir.join(name), res)?;
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
        "finished dump openrpc json for version: {:?} at dir: {:?}",
        version, json_dir
    );
    // run git commit all changes before generate rpc readme
    run_command("git", &["add", "."], Some(OPENRPC_DIR));
    run_command(
        "git",
        &[
            "commit",
            "-m",
            &format!("update openrpc json for version: {:?}", version),
        ],
        Some(OPENRPC_DIR),
    );
    // git push
    run_command("git", &["push"], Some(OPENRPC_DIR));
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

    let commit_sha = get_commit_sha();
    let generator = RpcDocGenerator::new(&all_rpc, readme_path.to_owned(), commit_sha);
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
