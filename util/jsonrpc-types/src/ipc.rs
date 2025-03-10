use crate::{OutPoint, Transaction, Uint64};
use ckb_types::H256;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a script locator for IPC requests.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct IpcScriptLocator {
    /// The out point of the script.
    pub out_point: Option<OutPoint>,
    /// The type ID of the script.
    pub type_id_args: Option<H256>,
}

/// Enum representing different payload formats for IPC requests and responses.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcPayloadFormat {
    /// Format is hexadecimal (hex).
    Hex,
    /// Format is JSON.
    #[default]
    Json,
}

/// Represents an IPC request with metadata and payload.
#[derive(Clone, Deserialize, JsonSchema, Serialize)]
pub struct IpcRequest {
    /// The version of the request.
    pub version: Uint64,
    /// The method ID of the request.
    pub method_id: Uint64,
    /// The format of the payload.
    pub payload_format: IpcPayloadFormat,
    /// The actual payload data in JSON format.
    pub payload: Value,
}

/// Represents an IPC response with metadata and payload.
#[derive(Clone, Deserialize, JsonSchema, Serialize)]
pub struct IpcResponse {
    /// The version of the response.
    pub version: Uint64,
    /// Error code associated with the response.
    pub error_code: Uint64,
    /// Format of the payload.
    pub payload_format: IpcPayloadFormat,
    /// Actual payload data in JSON format.
    pub payload: Value,
}

/// The script group type.
#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    /// Lock script group.
    Lock,
    /// Type script group.
    Type,
}

/// Represents the environment for IPC operations.
#[derive(Clone, Deserialize, JsonSchema, Serialize)]
pub struct IpcEnv {
    /// The transaction used in IPC operations.
    pub tx: Transaction,
    /// The type of script group used in IPC operations.
    pub script_group_type: ScriptGroupType,
    /// The script hash used in IPC operations.
    pub script_hash: H256,
}
