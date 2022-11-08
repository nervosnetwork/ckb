const SCRIPT_VERSION: crate::ScriptVersion = crate::ScriptVersion::V1;

#[path = "ckb_latest/features_since_v2019.rs"]
mod features_since_v2019;
#[path = "ckb_latest/features_since_v2021.rs"]
mod features_since_v2021;
#[path = "ckb_latest/features_since_v2023.rs"]
mod features_since_v2023;
