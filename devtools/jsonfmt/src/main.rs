use std::{
    env,
    fs::OpenOptions,
    io::{prelude::*, SeekFrom},
};

use serde::Serialize;
use serde_json::{ser::PrettyFormatter, Serializer, Value};

fn main() {
    let filepath = env::args().nth(1).expect("provide a json file");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&filepath)
        .unwrap_or_else(|err| panic!("failed to open the json file {}: {:?}", filepath, err));
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .unwrap_or_else(|err| panic!("failed to read the json file {}: {:?}", filepath, err));
    let value: Value =
        serde_json::from_str(&contents).unwrap_or_else(|err| panic!("malformed json: {:}", err));
    file.set_len(0)
        .unwrap_or_else(|err| panic!("failed to truncate the json file {}: {:?}", filepath, err));
    file.seek(SeekFrom::Start(0)).unwrap_or_else(|err| {
        panic!(
            "failed to seek the start position after truncate: {:?}",
            err
        )
    });
    let ident = b"    ";
    let formatter = PrettyFormatter::with_indent(&ident[..]);
    let mut serializer = Serializer::with_formatter(file, formatter);
    value
        .serialize(&mut serializer)
        .unwrap_or_else(|err| panic!("failed to rewrite the json file: {:}", err));
    let mut file = serializer.into_inner();
    file.write_all(b"\n")
        .unwrap_or_else(|err| panic!("failed to write a newline at end of file: {:}", err));
}
