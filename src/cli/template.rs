#![allow(dead_code)]

use super::signer::Signer;
use super::spec::Spec;
use std::io;
use std::path::Path;
use tera::Context;
use tera::Tera;

lazy_static! {
    pub static ref TEMPLATES: Templates = Templates::new();
}

pub trait TemplatesExt {
    type Output;

    fn load<P: AsRef<Path>>(path: P) -> io::Result<Self::Output>;

    fn write<P: AsRef<Path>>(&self, path: P) -> io::Result<()>;

    fn load_or_write_default<P: AsRef<Path>>(path: P) -> io::Result<Self::Output>;
}

pub struct Templates {
    inner: Tera,
}

impl Templates {
    pub fn new() -> Templates {
        let config_template = include_str!("templates/config.template");
        let signer_template = include_str!("templates/signer.template");

        let mut tera = Tera::default();
        tera.add_raw_templates(vec![
            ("config.toml", config_template),
            ("signer.toml", signer_template),
        ]).expect("Load templates");

        Templates { inner: tera }
    }

    pub fn render_spec(&self, spec: &Spec) -> String {
        let mut context = Context::new();
        context.add("logger", &spec.logger);
        context.add("network", &spec.network);
        context.add("rpc", &spec.rpc);
        self.inner
            .render("config.toml", &context)
            .expect("Render config")
    }

    pub fn render_signer(&self, signer: &Signer) -> String {
        let mut context = Context::new();
        context.add("private_key", &signer.private_key);
        self.inner
            .render("signer.toml", &context)
            .expect("Render signer")
    }
}

#[cfg(test)]
mod tests {
    use super::{Signer, Spec, Templates};

    #[test]
    fn render_spec() {
        let spec = Spec::default();
        println!("{}", Templates::new().render_spec(&spec));
    }

    #[test]
    fn render_signer() {
        let signer = Signer::default();
        println!("{}", Templates::new().render_signer(&signer));
    }
}
