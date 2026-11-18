//! Optional copies of controlled failure states, taken before recovery mutates them.
use std::{fs, path::Path};

pub(crate) fn capture(source: &Path, case: &str) {
    let Some(root) = std::env::var_os("CKB_RECOVERY_ARTIFACTS") else {
        return;
    };
    fs::create_dir_all(&root).unwrap();
    let destination = tempfile::Builder::new()
        .prefix(&format!("{case}-"))
        .tempdir_in(root)
        .unwrap()
        .keep();
    copy_directory(source, &destination.join("state"));
    println!("failure state {case}: {}", destination.display());
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
