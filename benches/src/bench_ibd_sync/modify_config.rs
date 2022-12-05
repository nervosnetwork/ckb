use std::fs;
use std::path::PathBuf;
use toml_edit::Document;

pub fn write_config<F>(filepath: PathBuf, mut f: F)
where
    F: FnMut(&mut Document),
{
    let content = fs::read_to_string(filepath.clone()).unwrap();

    let mut config = content.parse::<Document>().unwrap();
    f(&mut config);

    let tmp_filepath = filepath.with_extension(".tmp");
    fs::write(tmp_filepath.clone(), config.to_string()).unwrap();
    fs::rename(tmp_filepath, filepath).unwrap();
}

pub fn write_network_config(filepath: PathBuf, bootnodes: toml_edit::Array) {
    write_config(filepath, |config| {
        config["network"]["bootnodes"] = toml_edit::value(bootnodes.clone());
        config["network"]["discovery_local_address"] = toml_edit::value(true);
    })
}
