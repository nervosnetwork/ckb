use std::fs;
use std::path::{Path, PathBuf};

use crate::{Resource, TemplateContext, CKB_CONFIG_FILE_NAME};

fn mkdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ckb_resource_test")
        .tempdir()
        .unwrap()
}

fn touch<P: AsRef<Path>>(path: P) -> PathBuf {
    fs::create_dir_all(path.as_ref().parent().unwrap()).expect("create dir in test");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("touch file in test");

    path.as_ref().to_path_buf()
}

#[test]
fn test_exported_in() {
    let root_dir = mkdir();
    assert!(!Resource::exported_in(root_dir.path()));
    touch(root_dir.path().join(CKB_CONFIG_FILE_NAME));
    assert!(Resource::exported_in(root_dir.path()));
}

#[test]
fn test_export() {
    let root_dir = mkdir();
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
    Resource::bundled_ckb_config()
        .export(&context, root_dir.path())
        .expect("export ckb.toml");
    assert!(Resource::exported_in(root_dir.path()));
}
