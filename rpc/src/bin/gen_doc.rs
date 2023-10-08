//! this is a tool to generate rpc doc
use ckb_rpc::module::*;
use serde_json::Value;
use std::fs;

fn capitlize(s: &str) -> String {
    let mut res = String::new();
    res.push_str(&s[0..1].to_uppercase());
    res.push_str(&s[1..]);
    res
}

fn to_type(ty: &Value) -> String {
    match ty {
        Value::Object(map) => {
            if let Some(ty) = map.get("type") {
                if ty.as_str() == Some("array") {
                    format!("`Array<` {} `>`", to_type(&map["items"]))
                } else if ty.as_array().is_some() {
                    let ty = ty
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|ty| to_type(ty))
                        .collect::<Vec<_>>()
                        .join(" `|` ");
                    format!("`{}`", ty)
                } else {
                    format!("`{}`", ty.as_str().unwrap())
                }
            } else if map.get("anyOf").is_some() {
                map["anyOf"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|ty| to_type(ty))
                    .collect::<Vec<_>>()
                    .join(" `|` ")
            } else {
                let ty = map["$ref"].as_str().unwrap().split("/").last().unwrap();
                format!("[`{}`](#type-{})", ty, ty)
            }
        }
        _ => ty.as_str().unwrap().to_string(),
    }
}

fn to_ret_type(value: Option<&Value>) -> String {
    if let Some(value) = value {
        let ty = to_type(&value["schema"]);
        format!("* result: {}", ty)
    } else {
        "".to_owned()
    }
}

struct RpcModule {
    module_title: String,
    module_methods: Vec<serde_json::Value>,
}

impl RpcModule {
    fn to_string(&self) -> String {
        let mut res = String::new();
        let capitlized = capitlize(self.module_title.as_str());
        res.push_str(&format!(
            "    * [Module {}](#module-{})\n",
            capitlized, self.module_title
        ));
        for method in &self.module_methods {
            res.push_str(&format!(
                "        * [Method `{}`](#method-{})\n",
                method["name"].as_str().unwrap(),
                method["name"].as_str().unwrap()
            ));
        }
        res
    }

    fn to_content(&self) -> String {
        let mut res = String::new();
        let capitlized = capitlize(self.module_title.as_str());
        res.push_str(&format!("### Module {}\n", capitlized));

        for method in &self.module_methods {
            let name = method["name"].as_str().unwrap();
            // generate method signatures
            let args = method["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg["name"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join(", ");
            let arg_lines = method["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| {
                    let ty = to_type(&arg["schema"]);
                    format!("    * `{}`: {}", arg["name"].as_str().unwrap(), ty)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let ret_ty = to_ret_type(method.get("result"));
            let signatures = format!("* `{}({})`\n{}\n{}", name, args, arg_lines, ret_ty);
            let desc = method["description"].as_str().unwrap();
            let desc = desc.replace("##", "######");
            res.push_str(&format!(
                "#### Method `{}`\n{}\n\n{}\n",
                name, signatures, desc,
            ));
        }
        res
    }
}

fn dump_openrpc_json() -> Result<(), Box<dyn std::error::Error>> {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--json" {
        return dump_openrpc_json();
    }

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
    let mut rpc_module_methods = vec![];
    for rpc in all_rpc {
        match rpc {
            serde_json::Value::Object(map) => {
                let module_title = map["info"]["title"].as_str().unwrap();
                // strip `_rpc` suffix
                let module_title = &module_title[..module_title.len() - 4];
                let module_methods = map["methods"].as_array().unwrap();
                rpc_module_methods.push(RpcModule {
                    module_title: module_title.to_owned(),
                    module_methods: module_methods.to_owned(),
                });
            }
            _ => {}
        }
    }

    let readme = fs::read_to_string("./rpc/README.md")?;
    let lines = readme.lines().collect::<Vec<_>>();
    // strip lines below `**NOTE:**`
    for &line in lines.iter() {
        println!("{}", line);
        if line.contains("**NOTE:**") {
            break;
        }
    }

    println!("* [RPC Methods](#rpc-methods)");
    for rpc_module in rpc_module_methods.iter() {
        print!("{}", rpc_module.to_string());
    }

    for rpc_module in rpc_module_methods.iter() {
        println!("{}", rpc_module.to_content());
    }
    Ok(())
}
