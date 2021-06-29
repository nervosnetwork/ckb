use serde::{Deserialize, Serialize};

/// The CKB block chain edition.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ChainEdition {
    /// The CKB 2019 edition.
    #[serde(rename = "2019")]
    V2019,
    /// The CKB 2019 edition.
    #[serde(rename = "2021")]
    V2021,
}
