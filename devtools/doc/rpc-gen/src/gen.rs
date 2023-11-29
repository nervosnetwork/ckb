extern crate tera;
use ckb_rpc::RPCError;
use schemars::schema_for;
use serde_json::{Map, Value};
use std::{fs, vec};
use tera::Tera;

struct RpcModule {
    pub title: String,
    pub description: String,
    pub methods: Vec<serde_json::Value>,
}

impl RpcModule {
    pub fn gen_menu(&self, commit: &str) -> Value {
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
            (
                "link",
                gen_module_openrpc_playground(&capitlized, commit).into(),
            ),
        ])
    }

    pub fn gen_module_content(&self, commit: &str) -> String {
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
                let signatures = format!("* `{}({})`\n{}\n{}", name, args, arg_lines, ret_ty);
                let mut desc = method["description"]
                    .as_str()
                    .unwrap()
                    .replace("##", "######");
                desc = strip_prefix_space(&desc);
                format!("#### Method `{}`\n{}\n\n{}\n", name, signatures, desc,)
            })
            .collect::<Vec<_>>();

        render_tera(
            include_str!("../templates/module.tera"),
            &[
                ("name", capitlized.clone().into()),
                (
                    "link",
                    gen_module_openrpc_playground(&capitlized, commit).into(),
                ),
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
    commit: String,
}

impl RpcDocGenerator {
    pub fn new(all_rpc: &Vec<Value>, readme_path: String, commit: String) -> Self {
        let mut rpc_methods = vec![];
        let mut all_types: Vec<&Map<String, Value>> = vec![];
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
                let types = map["components"]["schemas"].as_object().unwrap();
                all_types.push(types);
                rpc_methods.push(RpcModule {
                    title,
                    description,
                    methods: methods.to_owned(),
                });
            }
        }

        // sort rpc_methods accoring to title
        rpc_methods.sort_by(|a, b| a.title.cmp(&b.title));

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
            rpc_methods,
            types,
            file_path: readme_path,
            commit,
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
            .map(|r| r.gen_menu(&self.commit))
            .collect::<Vec<_>>();

        let type_menus: Value = self
            .types
            .iter()
            .map(|(name, _)| vec![capitlize(name).into(), name.to_lowercase().into()])
            .collect::<Vec<Vec<Value>>>()
            .into();

        // generate module methods content
        let modules: Vec<Value> = self
            .rpc_methods
            .iter()
            .map(|r| r.gen_module_content(&self.commit).into())
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
                let fields = gen_type_fields(ty);
                gen_value(&[
                    ("name", capitlize(name).into()),
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
                } else if let Some(arr) = ty.as_array() {
                    let ty = arr
                        .iter()
                        .map(|t| format!("`{}`", gen_type(t)))
                        .collect::<Vec<_>>()
                        .join(" `|` ");
                    ty.to_string()
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
fn gen_module_openrpc_playground(module: &str, commit: &str) -> String {
    let title = format!("CKB-{}", capitlize(module));
    render_tera(
        include_str!("../templates/link.tera"),
        &[
            ("title", title.into()),
            ("module", module.to_lowercase().into()),
            ("commit", commit.into()),
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
