#![allow(clippy::redundant_closure)]

/// https://github.com/rust-lang/rust-clippy/pull/8431
// |    .unwrap_or_else(|err| std::panic::panic_any(err));
// |                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the function itself: `std::panic::panic_any`
use ckb_resource::{Resource, TemplateContext};

use crate::{app_config::*, cli};

fn mkdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("app_config_test")
        .tempdir()
        .unwrap()
}

#[test]
fn test_bundled_config_files() {
    let resource = Resource::bundled_ckb_config();
    CKBAppConfig::load_from_slice(&resource.get().expect("read bundled file"))
        .expect("deserialize config");

    let resource = Resource::bundled_miner_config();
    MinerAppConfig::load_from_slice(&resource.get().expect("read bundled file"))
        .expect("deserialize config");
}

#[test]
fn test_export_dev_config_files() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "true"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    {
        Resource::bundled_ckb_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let ckb_config = app_config
            .into_ckb()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            ckb_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("dev.toml"))
        );
        assert_eq!(
            ckb_config.network.listen_addresses,
            vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
        );
        assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
        assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
    }
    {
        Resource::bundled_miner_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_MINER)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let miner_config = app_config
            .into_miner()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(miner_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            miner_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("dev.toml"))
        );
        assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
    }
}

#[test]
fn test_log_to_stdout_only() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "false"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    {
        Resource::bundled_ckb_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let ckb_config = app_config
            .into_ckb()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert!(!ckb_config.logger.log_to_file);
        assert!(ckb_config.logger.log_to_stdout);
    }
    {
        Resource::bundled_miner_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_MINER)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let miner_config = app_config
            .into_miner()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert!(!miner_config.logger.log_to_file);
        assert!(miner_config.logger.log_to_stdout);
    }
}

#[test]
fn test_export_testnet_config_files() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "testnet",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "true"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    {
        Resource::bundled_ckb_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let ckb_config = app_config
            .into_ckb()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            ckb_config.chain.spec,
            Resource::bundled("specs/testnet.toml".to_string())
        );
        assert_eq!(
            ckb_config.network.listen_addresses,
            vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
        );
        assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
        assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
    }
    {
        Resource::bundled_miner_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_MINER)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let miner_config = app_config
            .into_miner()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(miner_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            miner_config.chain.spec,
            Resource::bundled("specs/testnet.toml".to_string())
        );
        assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
    }
}

#[test]
fn test_export_integration_config_files() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "integration",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "true"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    {
        Resource::bundled_ckb_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let ckb_config = app_config
            .into_ckb()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(
            ckb_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("integration.toml"))
        );
        assert_eq!(
            ckb_config.network.listen_addresses,
            vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
        );
        assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
    }
    {
        Resource::bundled_miner_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_MINER)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let miner_config = app_config
            .into_miner()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(
            miner_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("integration.toml"))
        );
        assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
    }
}

#[cfg(all(unix, target_pointer_width = "64"))]
#[test]
fn test_export_dev_config_files_assembly() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "true"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    {
        Resource::bundled_ckb_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let ckb_config = app_config
            .into_ckb()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            ckb_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("dev.toml"))
        );
        assert_eq!(
            ckb_config.network.listen_addresses,
            vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
        );
        assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
        assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
    }
    {
        Resource::bundled_miner_config()
            .export(&context, dir.path())
            .expect("export config files");
        let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_MINER)
            .unwrap_or_else(|err| std::panic::panic_any(err));
        let miner_config = app_config
            .into_miner()
            .unwrap_or_else(|err| std::panic::panic_any(err));
        assert_eq!(miner_config.logger.filter, Some("info".to_string()));
        assert_eq!(
            miner_config.chain.spec,
            Resource::file_system(dir.path().join("specs").join("dev.toml"))
        );
        assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
    }
}

#[test]
fn test_run_config_does_not_create_db_path_eagerly() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "false"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    Resource::bundled_ckb_config()
        .export(&context, dir.path())
        .expect("export config files");

    let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
        .unwrap_or_else(|err| std::panic::panic_any(err));
    let ckb_config = app_config
        .into_ckb()
        .unwrap_or_else(|err| std::panic::panic_any(err));
    assert!(ckb_config.data_dir.exists());
    assert!(
        !ckb_config.db.path.exists(),
        "run subcommand must not eagerly mkdir db.path before acquiring the rebuild lock"
    );
}

#[test]
fn test_migrate_config_does_not_create_db_path_eagerly() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "false"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    Resource::bundled_ckb_config()
        .export(&context, dir.path())
        .expect("export config files");

    let app_config = AppConfig::load_for_subcommand(dir.path(), "migrate")
        .unwrap_or_else(|err| std::panic::panic_any(err));
    let ckb_config = app_config
        .into_ckb()
        .unwrap_or_else(|err| std::panic::panic_any(err));
    assert!(ckb_config.data_dir.exists());
    assert!(
        !ckb_config.db.path.exists(),
        "migrate subcommand must not eagerly mkdir db.path before acquiring the rebuild lock"
    );
}

#[test]
fn test_config_defers_tmp_dir_and_ancient_under_db_path() {
    let dir = mkdir();
    let context = TemplateContext::new(
        "dev",
        vec![
            ("rpc_port", "7000"),
            ("p2p_port", "8000"),
            ("log_to_file", "false"),
            ("log_to_stdout", "true"),
            ("block_assembler", ""),
            ("spec_source", "bundled"),
        ],
    );
    Resource::bundled_ckb_config()
        .export(&context, dir.path())
        .expect("export config files");

    let ckb_toml_path = dir.path().join("ckb.toml");
    // Use uppercase `DB` to cover case-insensitive filesystems (macOS APFS / Windows NTFS)
    // where `data/DB/tmp` aliases `data/db/tmp` while `data/db` is absent mid-cutover.
    let nested_tmp = dir.path().join("data").join("DB").join("tmp");
    let nested_ancient = dir.path().join("data").join("DB").join("ancient");
    let nested_tmp_toml = toml::Value::String(nested_tmp.to_string_lossy().into_owned());
    let nested_ancient_toml = toml::Value::String(nested_ancient.to_string_lossy().into_owned());
    let original = std::fs::read_to_string(&ckb_toml_path).unwrap();
    let updated =
        format!("tmp_dir = {nested_tmp_toml}\nancient = {nested_ancient_toml}\n{original}");
    std::fs::write(&ckb_toml_path, updated).unwrap();

    let app_config = AppConfig::load_for_subcommand(dir.path(), cli::CMD_RUN)
        .unwrap_or_else(|err| std::panic::panic_any(err));
    let ckb_config = app_config
        .into_ckb()
        .unwrap_or_else(|err| std::panic::panic_any(err));
    assert!(
        !ckb_config.db.path.exists(),
        "loading config must not create db.path via nested tmp_dir or ancient"
    );
    assert!(!nested_tmp.exists());
    assert!(!nested_ancient.exists());
}
