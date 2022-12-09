use serde::{Deserialize, Serialize};
/// Notify config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// An executable script to be called whenever there's a new block in the canonical chain.
    ///
    /// The script is called with the block hash as the argument.
    pub new_block_notify_script: Option<String>,
    /// An executable script to be called whenever there's a new network alert received.
    ///
    /// The script is called with the alert message as the argument.
    pub network_alert_notify_script: Option<String>,

    /// Notify tx timeout in milliseconds
    #[serde(default, deserialize_with = "at_least_100")]
    pub notify_tx_timeout: Option<u64>,

    /// Notify alert timeout in milliseconds
    #[serde(default, deserialize_with = "at_least_100")]
    pub notify_alert_timeout: Option<u64>,

    /// Notify alert timeout in milliseconds
    #[serde(default, deserialize_with = "at_least_100")]
    pub script_timeout: Option<u64>,
}

fn at_least_100<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let op = Option::<u64>::deserialize(d)?;

    if let Some(ref value) = op {
        if value < &100 {
            return Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(*value),
                &"a value at least 100",
            ));
        }
    }
    Ok(op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let s = r#"
        new_block_notify_script = "dasd"
        network_alert_notify_script = "dasd"
        script_timeout = 1
        "#;

        let ret = toml::from_str::<Config>(s);
        assert!(ret.is_err());

        let s = r#"
        new_block_notify_script = "dasd"
        network_alert_notify_script = "dasd"
        script_timeout = 100
        "#;
        let ret = toml::from_str::<Config>(s);
        assert!(ret.is_ok());
    }
}
