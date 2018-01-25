#[macro_use]
extern crate clap;
#[macro_use]
extern crate log;
extern crate nervos_util as util;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate toml;

mod config;

use config::Config;
use util::logger;
use util::wait_for_exit;

fn main() {
    let matches = clap_app!(nervos =>
        (version: "0.1")
        (author: "Nervos <dev@nervos.org>")
        (about: "Nervos")
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file")
    ).get_matches();

    let config_path = matches.value_of("config").unwrap_or("default.toml");
    let config = Config::load(config_path);

    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}
