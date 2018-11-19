mod helper;
mod rsa;
mod run_impl;
mod signer;
mod spec;
mod template;

pub use self::run_impl::run;
pub use self::signer::Signer;
pub use self::spec::Spec;
pub use self::template::{Templates, TemplatesExt, TEMPLATES};
use clap::ArgMatches;

pub fn signer_cmd(matches: &ArgMatches) {
    if let Some(_matches) = matches.subcommand_matches("new") {
        Signer::gen_and_print();
    }
}
