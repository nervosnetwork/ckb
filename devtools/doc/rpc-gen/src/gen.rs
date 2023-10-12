use ckb_rpc::RPCError;
use schemars::schema_for;
use serde_json::{Map, Value};
use std::fs;

struct RpcModule {
    pub module_title: String,
    pub module_description: String,
    pub module_methods: Vec<serde_json::Value>,
}

impl RpcModule {
    pub fn gen_module_menu(&self) -> String {
        let mut res = String::new();
        let capitlized = self.module_title.to_string();
        res.push_str(&format!(
            "    * [Module {}](#module-{}) {}\n",
            capitlized,
            self.module_title.to_lowercase(),
            gen_module_openrpc_playground(&capitlized)
        ));
        let mut method_names = self
            .module_methods
            .iter()
            .map(|method| method["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        if capitlized == "Subscription" {
            method_names.push("subscribe");
            method_names.push("unsubscribe");
        }
        for name in method_names.iter() {
            res.push_str(&format!(
                "        * [Method `{}`](#method-{})\n",
                name, name
            ));
        }
        res
    }

    pub fn gen_module_content(&self) -> String {
        let mut res = String::new();
        let capitlized = self.module_title.to_string();
        let description = self.module_description.replace("##", "#####");

        res.push_str(&format!("### Module {}\n", capitlized));
        res.push_str(&format!(
            "- {}\n\n",
            gen_module_openrpc_playground(&capitlized)
        ));
        res.push_str(&format!("{}\n\n", description));

        for method in &self.module_methods {
            // generate method signatures
            let name = method["name"].as_str().unwrap();
            let params = method["params"].as_array().unwrap();
            let args = params
                .iter()
                .map(|arg| arg["name"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join(", ");
            let arg_lines = params
                .iter()
                .map(|arg| {
                    let ty = gen_type(&arg["schema"]);
                    format!("    * `{}`: {}", arg["name"].as_str().unwrap(), ty)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let ret_ty = if let Some(value) = method.get("result") {
                format!("* result: {}", gen_type(&value["schema"]))
            } else {
                "".to_string()
            };
            let signatures = format!("* `{}({})`\n{}\n{}", name, args, arg_lines, ret_ty);
            let mut desc = method["description"]
                .as_str()
                .unwrap()
                .replace("##", "######");
            desc = strip_prefix_space(&desc);
            res.push_str(&format!(
                "#### Method `{}`\n{}\n\n{}\n",
                name, signatures, desc,
            ));
        }
        res + "\n"
    }
}

pub(crate) struct RpcDocGenerator {
    rpc_module_methods: Vec<RpcModule>,
    types: Vec<(String, Value)>,
    file_path: String,
}

impl RpcDocGenerator {
    pub fn new(all_rpc: &Vec<Value>, readme_path: String) -> Self {
        let mut rpc_module_methods = vec![];
        let mut all_types: Vec<&Map<String, Value>> = vec![];
        for rpc in all_rpc {
            if let serde_json::Value::Object(map) = rpc {
                let module_title = capitlize(
                    map["info"]["title"]
                        .as_str()
                        .unwrap()
                        .trim_end_matches("_rpc"),
                );
                let module_description = get_description(&map["info"]["description"]);
                let module_methods = map["methods"].as_array().unwrap();
                let types = map["components"]["schemas"].as_object().unwrap();
                all_types.push(types);
                rpc_module_methods.push(RpcModule {
                    module_title,
                    module_description,
                    module_methods: module_methods.to_owned(),
                });
            }
        }

        // sort rpc_module_methods accoring to module_title
        rpc_module_methods.sort_by(|a, b| a.module_title.cmp(&b.module_title));

        let mut types: Vec<(String, Value)> = vec![];
        for map in all_types.iter() {
            for (name, ty) in map.iter() {
                if !types.iter().any(|(n, _)| *n == *name) {
                    types.push((name.to_string(), ty.to_owned()));
                }
            }
        }
        types.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

        Self {
            rpc_module_methods,
            types,
            file_path: readme_path,
        }
    }

    pub fn gen_markdown(self) -> String {
        let mut res = String::new();

        // strip lines below `**NOTE:**`
        let readme = fs::read_to_string(&self.file_path).unwrap_or("".to_string());
        let lines = readme.lines().collect::<Vec<_>>();
        for &line in lines.iter() {
            res.push_str(&(line.to_string() + "\n"));
            if line.contains("**NOTE:** the content below is generated by gen_doc") {
                break;
            }
        }

        // generate methods menu
        res.push_str("\n\n* [RPC Methods](#rpc-methods)\n");
        for rpc_module in self.rpc_module_methods.iter() {
            res.push_str(&rpc_module.gen_module_menu());
        }

        // generate type menu
        res.push_str("* [RPC Types](#rpc-types)\n");
        for (name, _) in self.types.iter() {
            let ty = format!(
                "    * [Type `{}`](#type-{})\n",
                capitlize(name),
                name.to_lowercase()
            );
            res.push_str(&ty);
        }
        // generate error menu
        res.push_str("* [RPC Errors](#rpc-errors)\n");

        // generate module methods content
        for rpc_module in self.rpc_module_methods.iter() {
            if rpc_module.module_title == "Subscription" {
                gen_subscription_rpc_doc(&mut res);
            } else {
                res.push_str(&rpc_module.gen_module_content());
            }
        }

        // generate type content
        res.push_str("## RPC Types\n");
        self.gen_type_content(&mut res);

        gen_errors_content(&mut res);
        res
    }

    fn gen_type_content(&self, res: &mut String) {
        for (name, ty) in self.types.iter() {
            let desc = if let Some(desc) = ty.get("description") {
                desc.as_str().unwrap().to_string()
            } else if let Some(desc) = ty.get("format") {
                format!("`{}` is `{}`", name, desc.as_str().unwrap())
            } else {
                "".to_string()
            };
            let desc = desc.replace("##", "######");
            // remove the inline code from comments
            let desc = desc
                .lines()
                .filter(|l| !l.contains("serde_json::from_str") && !l.contains(".unwrap()"))
                .collect::<Vec<_>>()
                .join("\n");

            // replace only the first ``` with ```json
            let desc = desc.replacen("```\n", "```json\n", 1);

            let fileds = gen_type_fields(ty);
            let type_desc = format!("### Type `{}`\n{}\n{}\n\n", capitlize(name), desc, fileds);
            res.push_str(&type_desc);
        }
    }
}

fn capitlize(s: &str) -> String {
    if s.is_empty() {
        return s.to_owned();
    }
    s[0..1].to_uppercase().to_string() + &s[1..]
}

fn strip_prefix_space(content: &str) -> String {
    let minimal_strip_count = content
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                usize::MAX
            } else {
                l.chars().take_while(|c| c.is_whitespace()).count()
            }
        })
        .min()
        .unwrap_or(0);
    if minimal_strip_count > 0 {
        content
            .lines()
            .map(|l| {
                if l.len() > minimal_strip_count {
                    l[minimal_strip_count..].to_string()
                } else {
                    "".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content.to_string()
    }
}

fn get_description(value: &Value) -> String {
    strip_prefix_space(value.as_str().unwrap())
}

fn gen_type_desc(desc: &str) -> String {
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
                l.to_string()
            };
            if l.is_empty() {
                l
            } else {
                format!("    {}", l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let desc = if left.is_empty() {
        first.to_string()
    } else {
        format!("{}\n\n{}", first, left)
    };
    format!(" - {}\n", desc)
}

fn gen_type_fields(ty: &Value) -> String {
    if let Some(fields) = ty.get("required") {
        let res = fields
            .as_array()
            .unwrap()
            .iter()
            .map(|field| {
                let field = field.as_str().unwrap();
                let field_desc = ty["properties"][field]["description"]
                    .as_str()
                    .map_or_else(|| "".to_string(), gen_type_desc);
                let ty_ref = gen_type(&ty["properties"][field]);
                format!("* `{}`: {}{}", field, ty_ref, field_desc)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let res = strip_prefix_space(&res);
        format!("\n#### Fields:\n{}", res)
    } else {
        "".to_string()
    }
}

fn gen_type(ty: &Value) -> String {
    match ty {
        Value::Object(map) => {
            if let Some(ty) = map.get("type") {
                if ty.as_str() == Some("array") {
                    // if `maxItems` is not set, then it's a fixed length array
                    // means it's a tuple, will be handled by `Value::Array` case
                    if map.get("maxItems").is_none() {
                        format!("`Array<` {} `>`", gen_type(&map["items"]))
                    } else {
                        gen_type(&map["items"])
                    }
                } else if ty.as_array().is_some() {
                    let ty = ty
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|t| format!("`{}`", gen_type(t)))
                        .collect::<Vec<_>>()
                        .join(" `|` ");
                    ty.to_string()
                } else {
                    format!("`{}`", ty.as_str().unwrap())
                }
            } else if map.get("anyOf").is_some() {
                map["anyOf"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(gen_type)
                    .collect::<Vec<_>>()
                    .join(" `|` ")
            } else {
                let ty = map["$ref"].as_str().unwrap().split('/').last().unwrap();
                format!("[`{}`](#type-{})", ty, ty.to_lowercase())
            }
        }
        Value::Array(arr) => {
            // the `tuple` case
            let elems = arr.iter().map(gen_type).collect::<Vec<_>>().join(" , ");
            format!("({})", elems)
        }
        Value::Null => "".to_string(),
        _ => ty.as_str().unwrap().to_string(),
    }
}

fn gen_errors_content(res: &mut String) {
    let schema = schema_for!(RPCError);
    let value = serde_json::to_value(schema).unwrap();
    let summary = get_description(&value["description"]);
    res.push_str("## RPC Errors\n");
    res.push_str(&summary);

    for error in value["oneOf"].as_array().unwrap().iter() {
        let desc = get_description(&error["description"]);
        let enum_ty = error["enum"].as_array().unwrap()[0].as_str().unwrap();
        let doc = format!("\n### ERROR `{}`\n{}\n", enum_ty, desc);
        res.push_str(&doc);
    }
}

/// generate subscription module, which is handled specially here
/// since jsonrpc-utils ignore the `SubscriptionRpc`
fn gen_subscription_rpc_doc(res: &mut String) {
    let pubsub_module_source = include_str!("../../../../rpc/src/module/subscription.rs");
    // read comments before `pub trait SubscriptionRpc` and treat it as module summary
    let summary = pubsub_module_source
        .lines()
        .take_while(|l| !l.contains("pub trait SubscriptionRpc"))
        .filter(|l| l.starts_with("///"))
        .map(|l| {
            l.trim_start()
                .trim_start_matches("///")
                .replacen(' ', "", 1)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary = strip_prefix_space(&summary);

    // read the continues comments between `S: Stream` and `fn subscribe`
    let sub_desc = pubsub_module_source
        .lines()
        .skip_while(|l| !l.contains("S: Stream"))
        .filter(|l| l.trim().starts_with("///"))
        .map(|l| {
            l.trim_start()
                .trim_start_matches("///")
                .replacen(' ', "", 1)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let sub_desc = strip_prefix_space(&sub_desc);

    res.push_str(format!("{}\n\n", summary).as_str());
    res.push_str(format!("{}\n", sub_desc).as_str());
}

/// generate openrpc playground urls
fn gen_module_openrpc_playground(module: &str) -> String {
    let logo = "https://raw.githubusercontent.com/chenyukang/ckb-rpc-resources/main/ckb-logo.jpg";
    let title = format!("CKB-{}", capitlize(module));
    let json_url = format!(
        "https://raw.githubusercontent.com/chenyukang/ckb-rpc-resources/main/json/{}_rpc_doc.json",
        module.to_lowercase()
    );

    format!("[👉 OpenRPC spec](http://playground.open-rpc.org/?uiSchema[appBar][ui:title]={}&uiSchema[appBar][ui:splitView]=false", title) +
    format!("&uiSchema[appBar][ui:examplesDropdown]=false&uiSchema[appBar][ui:logoUrl]={}&schemaUrl={})", logo, json_url).as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_prefix_space() {
        assert_eq!(strip_prefix_space("   "), "".to_string());
        assert_eq!(
            strip_prefix_space("   \n   abc\n   cdf"),
            "\nabc\ncdf".to_string()
        );
        assert_eq!(
            strip_prefix_space("abc\n   cde\n  cdf"),
            "abc\n   cde\n  cdf".to_string()
        );
    }
}
