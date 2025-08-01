extern crate tera;
use crate::syn::*;
use ckb_rpc::RPCError;
use schemars::schema_for;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::{fs, vec};
use tera::Tera;

struct RpcModule {
    pub title: String,
    pub description: String,
    pub methods: Vec<serde_json::Value>,
    pub deprecated: HashMap<String, (String, String)>,
}

impl RpcModule {
    pub fn gen_menu(&self) -> Value {
        let capitlized = self.title.to_string();
        let mut method_names = self
            .methods
            .iter()
            .map(|method| method["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        if capitlized == "Subscription" {
            method_names.push("subscribe".to_owned());
            method_names.push("unsubscribe".to_owned());
        }

        gen_value(&[
            ("title", capitlized.clone().into()),
            ("name", self.title.to_lowercase().into()),
            ("methods", method_names.into()),
            ("link", gen_module_openrpc_playground(&capitlized).into()),
        ])
    }

    pub fn gen_module_content(&self) -> String {
        if self.title == "Subscription" {
            return gen_subscription_rpc_doc();
        }
        let capitlized = self.title.to_string();
        let description = self.description.replace("##", "#####");

        let methods = self
            .methods
            .iter()
            .map(|method| {
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
                let fn_full_name = format!("{}.{}", self.title, name);
                let mut deprecated_desc = "".to_string();
                if let Some((version, desc)) = self.deprecated.get(&fn_full_name) {
                    deprecated_desc = format!("\n\n👎Deprecated since {}: {}\n", version, desc);
                }
                let signatures = format!(
                    "* `{}({})`\n{}\n{}{}",
                    name, args, arg_lines, ret_ty, deprecated_desc
                );
                let mut desc = method["description"]
                    .as_str()
                    .unwrap()
                    .replace("##", "######");
                desc = strip_prefix_space(&desc);
                let link = format!("<a id=\"{}-{}\"></a>", capitlized.to_lowercase(), name);
                format!(
                    "{}\n#### Method `{}`\n{}\n\n{}\n",
                    link, name, signatures, desc,
                )
            })
            .collect::<Vec<_>>();

        render_tera(
            include_str!("../templates/module.tera"),
            &[
                ("name", capitlized.clone().into()),
                ("link", gen_module_openrpc_playground(&capitlized).into()),
                ("desc", description.into()),
                ("methods", methods.into()),
            ],
        )
    }
}

pub(crate) struct RpcDocGenerator {
    rpc_methods: Vec<RpcModule>,
    types: Vec<(String, Value)>,
    file_path: String,
}

impl RpcDocGenerator {
    pub fn new(all_rpc: &Vec<Value>, readme_path: String) -> Self {
        let mut rpc_methods = vec![];
        let mut types: Vec<&Map<String, Value>> = vec![];

        let mut pre_defined: Vec<(String, String)> = pre_defined_types().collect();
        let finder = CommentFinder::new();
        let types_defined_in_source: Vec<(String, String)> = finder
            .type_comments
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let deprecated = finder.deprecated;
        pre_defined.extend(types_defined_in_source);

        for rpc in all_rpc {
            if let serde_json::Value::Object(map) = rpc {
                let title = capitlize(
                    map["info"]["title"]
                        .as_str()
                        .unwrap()
                        .trim_end_matches("_rpc"),
                );
                let description = get_description(&map["info"]["description"]);
                let methods = map["methods"].as_array().unwrap();
                let pair = map["components"]["schemas"].as_object().unwrap();
                types.push(pair);
                rpc_methods.push(RpcModule {
                    title,
                    description,
                    methods: methods.to_owned(),
                    deprecated: deprecated.clone(),
                });
            }
        }
        // sort rpc_methods according to title
        rpc_methods.sort_by(|a, b| a.title.cmp(&b.title));

        let mut all_types: Vec<(String, Value)> = pre_defined
            .iter()
            .map(|(name, desc)| (name.clone(), Value::String(desc.clone())))
            .collect();
        for map in types {
            for (name, ty) in map.iter() {
                if !(all_types.iter().any(|(n, _)| *n == *name)
                    || (name.starts_with("Either_for_") && name.ends_with("_JsonBytes")))
                {
                    all_types.push((name.to_string(), ty.to_owned()));
                }
            }
        }

        all_types.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));
        Self {
            rpc_methods,
            types: all_types,
            file_path: readme_path,
        }
    }

    pub fn gen_markdown(self) -> String {
        let readme = fs::read_to_string(&self.file_path).unwrap_or("".to_string());
        let lines = readme.lines().collect::<Vec<_>>();
        let summary: Value = lines
            .iter()
            .take_while(|l| !l.contains("**NOTE:** the content below is generated by gen_doc"))
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .into();

        // generate methods menu
        let module_menus = self
            .rpc_methods
            .iter()
            .map(|r| r.gen_menu())
            .collect::<Vec<_>>();

        let type_menus: Value = self
            .types
            .iter()
            .map(|(name, _)| vec![fix_type_name(name).into(), name.to_lowercase().into()])
            .collect::<Vec<Vec<Value>>>()
            .into();

        // generate module methods content
        let modules: Vec<Value> = self
            .rpc_methods
            .iter()
            .map(|r| r.gen_module_content().into())
            .collect::<Vec<_>>();

        let types = self.gen_type_contents();
        render_tera(
            include_str!("../templates/markdown.tera"),
            &[
                ("summary", summary),
                ("module_menus", module_menus.into()),
                ("type_menus", type_menus),
                ("modules", modules.into()),
                ("types", types.into()),
                ("errors", gen_errors_contents()),
            ],
        )
    }

    fn gen_type_contents(&self) -> Vec<Value> {
        self.types
            .iter()
            .map(|(name, ty)| {
                let desc = if let Some(desc) = ty.get("description") {
                    desc.as_str().unwrap().to_string()
                } else if let Some(desc) = ty.get("format") {
                    format!("`{}` is `{}`", name, desc.as_str().unwrap())
                } else {
                    ty.as_str().map_or_else(|| "".to_owned(), |v| v.to_owned())
                };
                let desc = desc.replace("##", "######");
                // remove the inline code from comments
                let desc = desc
                    .lines()
                    .filter(|l| !l.contains("serde_json::from_str") && !l.contains(".unwrap()"))
                    .collect::<Vec<_>>()
                    .join("\n");

                // replace only the first code snippet ``` with ```json
                let name = capitlize(name);
                let desc = desc.replacen("```\n", "```json\n", 1);
                let fields = gen_type_fields(&name, ty);
                let fixed_name = fix_type_name(&name);
                let sub_title = if fixed_name != name {
                    format!(
                        "<a id=\"type-{}\"></a>\n### Type `{}`",
                        name.to_lowercase(),
                        fixed_name
                    )
                } else {
                    format!("### Type `{}`", fixed_name)
                };
                gen_value(&[
                    ("sub_title", sub_title.into()),
                    ("name", fixed_name.into()),
                    ("desc", desc.into()),
                    ("fields", fields.into()),
                ])
            })
            .collect::<Vec<_>>()
    }
}

fn capitlize(s: &str) -> String {
    if s.is_empty() {
        return s.to_owned();
    }
    s[0..1].to_uppercase() + &s[1..]
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

// Fix type name issue caused by: https://github.com/GREsau/schemars/issues/193
fn fix_type_name(type_name: &str) -> String {
    let elems: Vec<_> = type_name.split("_for_").collect();
    let type_name = if elems.len() == 2 {
        format!("{}<{}>", fix_type_name(elems[0]), fix_type_name(elems[1]))
    } else {
        type_name.to_owned()
    };
    let elems: Vec<_> = type_name.split("_and_").collect();
    let type_name = if elems.len() == 2 {
        format!("{} | {}", fix_type_name(elems[0]), fix_type_name(elems[1]))
    } else {
        type_name.to_owned()
    };
    capitlize(&type_name)
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

fn format_fields(name: &str, fields: &str) -> String {
    format!(
        "\n#### Fields\n\n`{}` is a JSON object with the following fields.\n\n{}",
        fix_type_name(name),
        fields
    )
}

fn gen_type_fields(name: &str, ty: &Value) -> String {
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
        format_fields(name, &res)
    } else if let Some(properties) = ty.get("properties") {
        let properties = properties.as_object().unwrap();
        let res = properties
            .iter()
            .map(|(key, value)| {
                let ty_ref = gen_type(value.get("items").unwrap_or(value));
                let field_desc = value.get("description").unwrap().as_str().unwrap();
                let field_desc = field_desc
                    .split('\n')
                    .map(|l| {
                        let l = l.trim();
                        if !l.is_empty() {
                            format!("    {}", l)
                        } else {
                            l.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("* `{}`: {} {}", key, ty_ref, field_desc.trim())
            })
            .collect::<Vec<_>>()
            .join("\n");
        format_fields(name, &res)
    } else if let Some(_values) = ty.get("oneOf") {
        gen_type(ty)
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
                } else if ty.as_str() == Some("string") {
                    let mut enum_val = String::new();
                    let mut desc = String::new();
                    if let Some(arr) = map.get("enum") {
                        enum_val = arr.as_array().unwrap()[0].as_str().unwrap().to_owned();
                    }
                    if let Some(val) = map.get("description") {
                        desc = val.as_str().unwrap_or_default().to_owned();
                    }

                    if !enum_val.is_empty() && !desc.is_empty() {
                        format!("  - {} : {}", enum_val, desc)
                    } else {
                        format!("`{}`", ty.as_str().unwrap())
                    }
                } else if let Some(arr) = ty.as_array() {
                    let ty = arr
                        .iter()
                        .map(|t| format!("`{}`", gen_type(t)))
                        .collect::<Vec<_>>()
                        .join(" `|` ");
                    ty
                } else if ty.as_str() == Some("object") {
                    // json schemars bug!
                    // type is `HashMap` here
                    "".to_string()
                } else {
                    format!("`{}`", ty.as_str().unwrap())
                }
            } else if let Some(arr) = map.get("anyOf") {
                arr.as_array()
                    .unwrap()
                    .iter()
                    .map(gen_type)
                    .collect::<Vec<_>>()
                    .join(" `|` ")
            } else if let Some(arr) = map.get("oneOf") {
                let mut res = arr
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(gen_type)
                    .collect::<Vec<_>>();
                res.retain(|value| value != "`string`");
                format!("\nIt's an enum value from one of:\n{}\n", res.join("\n"))
            } else if let Some(link) = map.get("$ref") {
                let link = link.as_str().unwrap().split('/').last().unwrap();
                format!("[`{}`](#type-{})", fix_type_name(link), link.to_lowercase())
            } else {
                "".to_owned()
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

fn gen_errors_contents() -> Value {
    let schema = schema_for!(RPCError);
    let value = serde_json::to_value(schema).unwrap();
    let summary = get_description(&value["description"]);
    let errors: Vec<Value> = value["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| {
            let desc = get_description(&error["description"]);
            let enum_ty = error["enum"].as_array().unwrap()[0].as_str().unwrap();
            vec![enum_ty.to_string(), desc].into()
        })
        .collect::<Vec<_>>();
    gen_value(&[("summary", summary.into()), ("errors", errors.into())])
}

/// generate subscription module, which is handled specially here
/// since jsonrpc-utils ignore the `SubscriptionRpc`
fn gen_subscription_rpc_doc() -> String {
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

    format!("{}\n\n{}\n", summary, sub_desc)
}

/// generate openrpc playground urls
fn gen_module_openrpc_playground(module: &str) -> String {
    let title = format!("CKB-{}", capitlize(module));
    render_tera(
        include_str!("../templates/link.tera"),
        &[
            ("title", title.into()),
            ("module", module.to_lowercase().into()),
        ],
    )
}

/// wrapper for render value
fn gen_value(pairs: &[(&str, Value)]) -> Value {
    let mut res = Value::Object(Map::new());
    for (k, v) in pairs {
        res.as_object_mut()
            .unwrap()
            .insert(k.to_string(), v.to_owned());
    }
    res
}

fn render_tera(template: &str, content: &[(&str, Value)]) -> String {
    let mut context = tera::Context::new();
    for (k, v) in content {
        context.insert(*k, v);
    }
    let mut tera = Tera::default();
    tera.add_raw_template("template", template).unwrap();
    tera.render("template", &context).unwrap()
}

fn pre_defined_types() -> impl Iterator<Item = (String, String)> {
    [
        ("AlertId", "The alert identifier that is used to filter duplicated alerts.\n
This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](#type-uint32)."),
        ("AlertPriority", "Alerts are sorted by priority. Greater integers mean higher priorities.\n
This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](#type-uint32)."),
        ("EpochNumber", "Consecutive epoch number starting from 0.\n
This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64)."),
        ("SerializedHeader", "This is a 0x-prefix hex string. It is the block header serialized by molecule using the schema `table Header`."),
        ("SerializedBlock", "This is a 0x-prefix hex string. It is the block serialized by molecule using the schema `table Block`."),
        ("U256", "The 256-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON."),
        ("H256", "The 256-bit binary data encoded as a 0x-prefixed hex string in JSON."),
        ("Byte32", "The fixed-length 32 bytes binary encoded as a 0x-prefixed hex string in JSON."),
        ("RationalU256", "The ratio which numerator and denominator are both 256-bit unsigned integers.")
    ].iter().map(|&(x, y)| (x.to_string(), y.to_string()))
}
