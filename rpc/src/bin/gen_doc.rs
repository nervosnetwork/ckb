//! this is a tool to generate rpc doc
use ckb_rpc::module::*;
use serde_json::{Map, Value};
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
                        .map(to_type)
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
                    .map(to_type)
                    .collect::<Vec<_>>()
                    .join(" `|` ")
            } else {
                let ty = map["$ref"].as_str().unwrap().split('/').last().unwrap();
                format!("[`{}`](#type-{})", ty, ty)
            }
        }
        Value::Null => "".to_owned(),
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
    types: Map<String, Value>,
}

impl RpcModule {
    fn to_menu(&self) -> String {
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

fn format_type_field(desc: &str) -> String {
    // split desc by "\n\n" and only keep the first line
    // then add extra leading space for left lines
    let split = desc.split("\n\n");
    let first = if let Some(line) = split.clone().next() {
        line
    } else {
        desc
    };
    let left = split.skip(1).collect::<Vec<_>>().join("\n\n");
    // add extra leading space for left lines
    let left = left
        .lines()
        .map(|l| {
            let l = l.trim_start();
            let l = if l.starts_with('#') {
                format!("**{}**", l.trim().trim_matches('#').trim())
            } else {
                l.to_owned()
            };
            format!("      {}", l)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let desc = if left.is_empty() {
        first.to_owned()
    } else {
        format!("{}\n\n{}", first, left)
    };
    format!(" - {}\n", desc)
}

fn get_type_fields(ty: &Value) -> String {
    if let Some(fields) = ty.get("required") {
        let fields = fields
            .as_array()
            .unwrap()
            .iter()
            .map(|field| {
                let field = field.as_str().unwrap();
                let field_desc = ty["properties"][field]["description"]
                    .as_str()
                    .map_or_else(|| "".to_owned(), format_type_field);
                let ty = to_type(&ty["properties"][field]["schema"]);
                format!("    * `{}`: {}{}", field, ty, field_desc)
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("* fields:\n{}", fields)
    } else {
        "".to_owned()
    }
}

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
    eprintln!("dump openrpc json...");
    Ok(())
}

fn gen_rpc_readme() -> Result<(), Box<dyn std::error::Error>> {
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
        if let serde_json::Value::Object(map) = rpc {
            let module_title = map["info"]["title"].as_str().unwrap();
            // strip `_rpc` suffix
            let module_title = &module_title[..module_title.len() - 4];
            let module_methods = map["methods"].as_array().unwrap();
            let types = map["components"]["schemas"].as_object().unwrap();
            rpc_module_methods.push(RpcModule {
                module_title: module_title.to_owned(),
                module_methods: module_methods.to_owned(),
                types: types.to_owned(),
            });
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

    let mut types: Vec<(String, &Value)> = vec![];
    for rpc_module in rpc_module_methods.iter() {
        for (name, ty) in rpc_module.types.iter() {
            if !types.iter().any(|(n, _)| *n == *name) {
                types.push((name.to_owned(), ty));
            }
        }
    }
    // sort according to name
    types.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

    // generate methods menu
    println!("* [RPC Methods](#rpc-methods)");
    for rpc_module in rpc_module_methods.iter() {
        print!("{}", rpc_module.to_menu());
    }

    // generate type menu
    println!("* [RPC Types](#rpc-types)");
    for (name, _) in types.iter() {
        println!("    * [Type `{}`](#type-{})", capitlize(name), name);
    }

    // generate methods content
    for rpc_module in rpc_module_methods.iter() {
        println!("{}", rpc_module.to_content());
    }

    // generate type content
    println!("## RPC Types");
    for (name, ty) in types.iter() {
        let desc = if let Some(desc) = ty.get("description") {
            desc.as_str().unwrap().to_owned()
        } else if let Some(desc) = ty.get("format") {
            format!("`{}` is `{}`", name, desc.as_str().unwrap())
        } else {
            "".to_owned()
        };
        let desc = desc.replace("##", "######");
        let desc = desc
            .lines()
            .filter(|l| !l.contains("serde_json::from_str") && !l.contains(".unwrap()"))
            .collect::<Vec<_>>()
            .join("\n");
        // replace only the first ``` with ```json
        let desc = desc.replacen("```", "```json", 1);

        let fileds = get_type_fields(ty);
        println!("### Type `{}`\n{}\n{}\n", capitlize(name), desc, fileds);
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--json" {
        return dump_openrpc_json();
    }
    gen_rpc_readme()?;
    Ok(())
}
