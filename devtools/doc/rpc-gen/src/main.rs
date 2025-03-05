//! this is a tool to generate rpc doc
mod r#gen;
mod syn;
mod utils;
use crate::r#gen::RpcDocGenerator;
use crate::utils::*;
use serde_json::json;
use std::{fs, path::PathBuf};

fn dump_openrpc_json() -> Result<(), Box<dyn std::error::Error>> {
    let json_dir = PathBuf::from(OPENRPC_DIR).join("json");
    let version = get_version();
    let branch = get_current_git_branch();
    checkout_openrpc_branch(&branch);
    fs::create_dir_all(&json_dir)?;

    for (name, mut doc) in all_rpc_docs() {
        doc["info"]["version"] = serde_json::Value::String(version.clone());
        let obj = json!(doc);
        let res = serde_json::to_string_pretty(&obj)?;
        fs::write(json_dir.join(name), res)?;
    }
    eprintln!(
        "finished dump openrpc json for version: {:?} at dir: {:?}",
        version, json_dir
    );

    if is_git_repo_dirty() {
        // run git commit all changes before generate rpc readme
        eprintln!("commit OpenRPC changes to repo: {}", get_git_remote_url());
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
        run_command("git", &["push"], Some(OPENRPC_DIR));
    }
    Ok(())
}

/// Generate rpc readme
fn gen_rpc_readme(readme_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let rpc_docs = all_rpc_docs()
        .iter()
        .map(|(_, doc)| doc.clone())
        .collect::<Vec<_>>();
    let generator = RpcDocGenerator::new(&rpc_docs, readme_path.to_owned());
    fs::write(readme_path, generator.gen_markdown())?;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--json") => dump_openrpc_json(),
        Some(readme_path) => gen_rpc_readme(readme_path),
        None => Ok(()),
    }
}
