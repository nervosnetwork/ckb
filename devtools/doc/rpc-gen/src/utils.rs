use ckb_rpc::module::*;
use serde_json::Value;

pub const OPENRPC_DIR: &str = "./docs/ckb_rpc_openrpc/";

macro_rules! generate_docs {
    ($($func:ident),* $(,)?) => {
        [
            $(
                (format!("{}.json", stringify!($func)), $func()),
            )*
        ]
    };
}

pub(crate) fn all_rpc_docs() -> Vec<(String, Value)> {
    generate_docs!(
        alert_rpc_doc,
        net_rpc_doc,
        subscription_rpc_doc,
        debug_rpc_doc,
        chain_rpc_doc,
        miner_rpc_doc,
        pool_rpc_doc,
        stats_rpc_doc,
        integration_test_rpc_doc,
        indexer_rpc_doc,
        experiment_rpc_doc,
    )
    .into()
}

pub(crate) fn run_command(prog: &str, args: &[&str], dir: Option<&str>) -> Option<String> {
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

pub(crate) fn get_version() -> String {
    let version = run_command("cargo", &["pkgid"], None)
        .unwrap()
        .split('#')
        .nth(1)
        .unwrap_or("0.0.0")
        .to_owned();
    version
}

pub(crate) fn get_commit_sha() -> String {
    let res = run_command("git", &["rev-parse", "HEAD"], Some(OPENRPC_DIR)).unwrap();
    eprintln!("commit sha: {:?}", res);
    res
}

pub(crate) fn checkout_tag_branch(version: &str) {
    let dir = Some(OPENRPC_DIR);
    let res = run_command("git", &["checkout", version], dir);
    if res.is_none() {
        run_command("git", &["checkout", "-b", version], dir);
    }
}
