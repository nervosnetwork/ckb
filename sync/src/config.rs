pub enum VerificationLevel {
    /// Full verification.
    Full,
    /// Transaction scripts are not checked.
    Header,
    /// No verification at all.
    Noop,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    // other config
    pub verification_level: String,
    pub orphan_block_limit: usize,
}

impl Config {
    pub fn default() -> Self {
        Config {
            verification_level: "Full".to_owned(),
            orphan_block_limit: 1024,
        }
    }
}
