//! this is a tool to generate rpc doc
mod r#gen;
mod syn;
mod utils;
use crate::r#gen::RpcDocGenerator;
use crate::utils::*;
use serde_json::json;
use std::{fs, path::PathBuf};

fn titlecase_tag(title: &str) -> String {
    let mut base = title.to_string();
    if base.ends_with("_rpc") {
        let new_len = base.len().saturating_sub(4);
        base.truncate(new_len);
    }
    base.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = String::new();
                    out.push(first.to_ascii_uppercase());
                    out.push_str(&chars.as_str().to_ascii_lowercase());
                    out
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

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

fn dump_openrpc_json_combined() -> Result<(), Box<dyn std::error::Error>> {
    let json_dir = PathBuf::from(OPENRPC_DIR).join("json");
    let version = get_version();
    let branch = get_current_git_branch();
    checkout_openrpc_branch(&branch);
    fs::create_dir_all(&json_dir)?;

    let mut methods = vec![];
    let mut tags = std::collections::BTreeSet::new();
    let mut schemas = serde_json::Map::new();

    for (_name, doc) in all_rpc_docs() {
        let info_title = doc
            .get("info")
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("rpc");
        let tag_name = titlecase_tag(info_title);
        tags.insert(tag_name.clone());

        if let Some(doc_methods) = doc.get("methods").and_then(|v| v.as_array()) {
            for method in doc_methods {
                let mut method = method.clone();
                let mut method_tags = method
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                method_tags.push(json!({ "name": tag_name }));
                method["tags"] = serde_json::Value::Array(method_tags);
                methods.push(method);
            }
        }

        if let Some(doc_schemas) = doc
            .get("components")
            .and_then(|v| v.get("schemas"))
            .and_then(|v| v.as_object())
        {
            for (k, v) in doc_schemas {
                schemas.insert(k.clone(), v.clone());
            }
        }
    }

    let tag_list = tags
        .into_iter()
        .map(|name| json!({ "name": name }))
        .collect::<Vec<_>>();
    let combined = json!({
        "openrpc": "1.2.6",
        "info": {
            "title": "CKB RPC",
            "version": version,
        },
        "methods": methods,
        "tags": tag_list,
        "components": {
            "schemas": schemas,
        },
    });

    let res = serde_json::to_string_pretty(&combined)?;
    fs::write(json_dir.join("ckb_rpc.json"), res)?;

    eprintln!(
        "finished dump combined openrpc json for version: {:?} at dir: {:?}",
        version, json_dir
    );

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
        Some("--json-all") => dump_openrpc_json_combined(),
        Some(readme_path) => gen_rpc_readme(readme_path),
        None => Ok(()),
    }
}
