use ckb_resource::{Resource, TemplateContext, AVAILABLE_SPECS};

use crate::{
    deprecate,
    legacy::{CKBAppConfig, DeprecatedField, MinerAppConfig},
};

fn mkdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ckb_app_config_test")
        .tempdir()
        .unwrap()
}

#[test]
fn macro_deprecate_works_well() {
    struct Config {
        first: Option<usize>,
        second: AConfig,
    }
    struct AConfig {
        a_f1: Option<usize>,
        a_f2: BConfig,
    }
    struct BConfig {
        b_f1: Option<usize>,
    }

    let c = Config {
        first: Some(0),
        second: AConfig {
            a_f1: Some(1),
            a_f2: BConfig { b_f1: Some(2) },
        },
    };
    let deprecated_fields = {
        let mut v = Vec::new();
        deprecate!(c, v, first, "0.1.0");
        deprecate!(c, v, second.a_f1, "0.2.0");
        deprecate!(c, v, second.a_f2.b_f1, "0.3.0");
        v
    };
    assert_eq!(deprecated_fields.len(), 3);
    assert_eq!(deprecated_fields[0].path, "first");
    assert_eq!(deprecated_fields[1].path, "second.a_f1");
    assert_eq!(deprecated_fields[2].path, "second.a_f2.b_f1");
}

#[test]
fn no_deprecated_fields_in_bundled_ckb_app_config() {
    let root_dir = mkdir();
    for name in AVAILABLE_SPECS {
        let context = TemplateContext::new(
            name,
            vec![
                ("rpc_port", "7000"),
                ("p2p_port", "8000"),
                ("log_to_file", "true"),
                ("log_to_stdout", "true"),
                ("block_assembler", ""),
                ("spec_source", "bundled"),
            ],
        );
        Resource::bundled_ckb_config()
            .export(&context, root_dir.path())
            .expect("export ckb.toml");
        let resource = Resource::ckb_config(root_dir.path());
        let legacy_config: CKBAppConfig =
            toml::from_slice(&resource.get().expect("resource get slice"))
                .expect("toml load slice");
        assert!(legacy_config.deprecated_fields().is_empty());
    }
}

#[test]
fn no_deprecated_fields_in_bundled_miner_app_config() {
    let root_dir = mkdir();
    for name in AVAILABLE_SPECS {
        let context = TemplateContext::new(
            name,
            vec![
                ("log_to_file", "true"),
                ("log_to_stdout", "true"),
                ("spec_source", "bundled"),
            ],
        );
        Resource::bundled_miner_config()
            .export(&context, root_dir.path())
            .expect("export ckb-miner.toml");
        let resource = Resource::miner_config(root_dir.path());
        let legacy_config: MinerAppConfig =
            toml::from_slice(&resource.get().expect("resource get slice"))
                .expect("toml load slice");
        assert!(legacy_config.deprecated_fields().is_empty());
    }
}
