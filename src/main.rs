#[macro_use]
extern crate clap;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod config;

fn main() {
    let matches = clap_app!(nervos =>
        (version: "0.1")
        (author: "Nervos <dev@nervos.org>")
        (about: "Nervos")
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file")
    ).get_matches();

    let config = matches.value_of("config").unwrap_or("default.toml");
    println!("Value for config: {}", config);
}
