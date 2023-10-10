use serde_json::{Map, Value};

pub(crate) fn capitlize(s: &str) -> String {
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
                    // if `maxItems` is not set, then it's a fixed length array
                    // means it's a tuple, will be handled by `Value::Array` case
                    if map.get("maxItems").is_none() {
                        format!("`Array<` {} `>`", to_type(&map["items"]))
                    } else {
                        to_type(&map["items"])
                    }
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
                format!("[`{}`](#type-{})", ty, ty.to_lowercase())
            }
        }
        Value::Array(arr) => {
            // the `tuple` case
            let elems = arr.iter().map(to_type).collect::<Vec<_>>().join(" , ");
            format!("({})", elems)
        }
        Value::Null => "".to_owned(),
        _ => ty.as_str().unwrap().to_string(),
    }
}

fn gen_ret_type(value: Option<&Value>) -> String {
    if let Some(value) = value {
        let ty = to_type(&value["schema"]);
        format!("* result: {}", ty)
    } else {
        "".to_owned()
    }
}

pub(crate) struct RpcModule {
    pub module_title: String,
    pub module_methods: Vec<serde_json::Value>,
    pub types: Map<String, Value>,
}

impl RpcModule {
    pub fn gen_methods_menu(&self) -> String {
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

    pub fn gen_methods_content(&self) -> String {
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
            let ret_ty = gen_ret_type(method.get("result"));
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
                l.to_owned()
            };
            format!("    {}", l)
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

fn gen_type_fields(ty: &Value) -> String {
    if let Some(fields) = ty.get("required") {
        let fields = fields
            .as_array()
            .unwrap()
            .iter()
            .map(|field| {
                let field = field.as_str().unwrap();
                let field_desc = ty["properties"][field]["description"]
                    .as_str()
                    .map_or_else(|| "".to_owned(), gen_type_desc);
                let ty_ref = to_type(&ty["properties"][field]);
                format!("* `{}`: {}{}", field, ty_ref, field_desc)
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("#### Fields:\n{}", fields)
    } else {
        "".to_owned()
    }
}

pub(crate) fn gen_type_content(res: &mut String, types: Vec<(String, &Value)>) {
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
        let desc = desc.replacen("```\n", "```json\n", 1);

        let fileds = gen_type_fields(ty);
        let type_desc = format!("### Type `{}`\n{}\n{}\n\n", capitlize(name), desc, fileds);
        res.push_str(&type_desc);
    }
}
