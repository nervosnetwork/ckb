use serde_json::{Value, json};
use std::{collections::HashSet, env, fs, path::PathBuf};

fn subscribe_method() -> Value {
    let description = r#" Subscribes to a topic.

 ## Params

 * `topic` - Subscription topic (enum: new_tip_header | new_tip_block | new_transaction | proposed_transaction | rejected_transaction)

 ## Returns

 This RPC returns the subscription ID as the result. CKB node will push messages in the subscribed topics to the current RPC connection. The subscription ID is attached as `params.subscription` in push messages.

 ## Examples

 Subscribe Request

 ```json
 {
   "id": 42,
   "jsonrpc": "2.0",
   "method": "subscribe",
   "params": [
     "new_tip_header"
   ]
 }
 ```

 Subscribe Response

 ```json
 {
   "id": 42,
   "jsonrpc": "2.0",
   "result": "0x2a"
 }
 ```
"#;

    json!({
        "name": "subscribe",
        "description": description,
        "params": [
            {
                "name": "topic",
                "schema": {
                    "type": "string",
                    "enum": [
                        "new_tip_header",
                        "new_tip_block",
                        "new_transaction",
                        "proposed_transaction",
                        "rejected_transaction"
                    ]
                }
            }
        ],
        "result": {
            "name": "result",
            "schema": { "type": "string" }
        },
        "tags": [{ "name": "Subscription" }]
    })
}

fn unsubscribe_method() -> Value {
    let description = r#" Unsubscribes from a subscribed topic.

 ## Params

 * `id` - Subscription ID

 ## Returns

 `true` if successfully unsubscribed.

 ## Examples

 Unsubscribe Request

 ```json
 {
   "id": 42,
   "jsonrpc": "2.0",
   "method": "unsubscribe",
   "params": [
     "0x2a"
   ]
 }
 ```

 Unsubscribe Response

 ```json
 {
   "id": 42,
   "jsonrpc": "2.0",
   "result": true
 }
 ```
"#;

    json!({
        "name": "unsubscribe",
        "description": description,
        "params": [
            {
                "name": "id",
                "schema": { "type": "string" }
            }
        ],
        "result": {
            "name": "result",
            "schema": { "type": "boolean" }
        },
        "tags": [{ "name": "Subscription" }]
    })
}

fn ensure_subscription_tag(doc: &mut Value) {
    if !doc.get("tags").and_then(|v| v.as_array()).is_some() {
        doc["tags"] = Value::Array(vec![]);
    }
    let tags = doc["tags"].as_array_mut().unwrap();
    let has = tags
        .iter()
        .any(|tag| tag.get("name").and_then(Value::as_str) == Some("Subscription"));
    if !has {
        tags.push(json!({ "name": "Subscription" }));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/ckb_rpc_openrpc/json/ckb_rpc.json"));

    let raw = fs::read_to_string(&path)?;
    let mut doc: Value = serde_json::from_str(&raw)?;

    ensure_subscription_tag(&mut doc);

    if !doc.get("methods").and_then(|v| v.as_array()).is_some() {
        doc["methods"] = Value::Array(vec![]);
    }

    let methods = doc["methods"].as_array_mut().unwrap();
    let names: HashSet<&str> = methods
        .iter()
        .filter_map(|m| m.get("name").and_then(Value::as_str))
        .collect();

    if !names.contains("subscribe") {
        methods.push(subscribe_method());
    }
    if !names.contains("unsubscribe") {
        methods.push(unsubscribe_method());
    }

    fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")?;
    Ok(())
}
