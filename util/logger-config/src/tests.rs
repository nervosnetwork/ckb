use crate::{Config, ExtraLoggerConfig};

fn update_extra_logger(config: &mut Config, name: &str, filter: &str) {
    let value = ExtraLoggerConfig {
        filter: filter.to_owned(),
    };
    config.extra.insert(name.to_owned(), value);
}

#[test]
fn test_default_params() {
    let config: Config = toml::from_str("").unwrap();
    let expected = Config::default();
    assert_eq!(config, expected);
}

#[test]
fn test_extra_loggers() {
    let config: Config = toml::from_str(
        r#"
            [extra.errors]
            filter = "error"
            [extra.ckb_trace]
            filter = "off,ckb=trace"
        "#,
    )
    .unwrap();
    let expected = {
        let mut config = Config::default();
        update_extra_logger(&mut config, "errors", "error");
        update_extra_logger(&mut config, "ckb_trace", "off,ckb=trace");
        config
    };
    assert_eq!(config, expected);
}
