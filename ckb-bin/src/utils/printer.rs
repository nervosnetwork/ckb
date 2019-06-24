use std::env;
use std::fmt;

use atty;
use colored::Colorize;

use crate::utils::json_color::Colorizer;
use crate::utils::yaml_ser;

pub fn is_a_tty(stderr: bool) -> bool {
    let stream = if stderr {
        atty::Stream::Stderr
    } else {
        atty::Stream::Stdout
    };
    atty::is(stream)
}

pub fn is_term_dumb() -> bool {
    env::var("TERM").ok() == Some(String::from("dumb"))
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    Yaml,
    Json,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match self {
                OutputFormat::Yaml => "yaml",
                OutputFormat::Json => "json",
            }
        )
    }
}

impl OutputFormat {
    pub fn from_str(format: &str) -> Result<OutputFormat, String> {
        match format {
            "yaml" => Ok(OutputFormat::Yaml),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("Invalid output format: {}", format)),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

impl Default for ColorWhen {
    fn default() -> Self {
        let is_a_tty = is_a_tty(false);
        let is_term_dumb = is_term_dumb();
        if is_a_tty && !is_term_dumb {
            ColorWhen::Auto
        } else {
            ColorWhen::Never
        }
    }
}

impl ColorWhen {
    pub fn new(color: bool) -> ColorWhen {
        let is_a_tty = is_a_tty(false);
        let is_term_dumb = is_term_dumb();
        if is_a_tty && !is_term_dumb && color {
            ColorWhen::Always
        } else {
            ColorWhen::Never
        }
    }

    pub fn color(self) -> bool {
        self != ColorWhen::Never
    }
}

pub trait Printable {
    fn render(&self, format: OutputFormat, color: bool) -> String;
}

impl Printable for Box<dyn Printable> {
    fn render(&self, format: OutputFormat, color: bool) -> String {
        self.as_ref().render(format, color)
    }
}

impl<T: ?Sized> Printable for T
where
    T: serde::ser::Serialize,
{
    fn render(&self, format: OutputFormat, color: bool) -> String {
        match format {
            OutputFormat::Yaml => yaml_ser::to_string(self, color).unwrap(),
            OutputFormat::Json => {
                let value = serde_json::to_value(self).unwrap();
                if color {
                    Colorizer::arbitrary().colorize_json_value(&value).unwrap()
                } else {
                    serde_json::to_string_pretty(&value).unwrap()
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum TypedStr<'a> {
    Null(Option<&'a str>),
    Bool(&'a str),
    Number(&'a str),
    String(&'a str),
    Key(&'a str),
    Escaped(&'a str),
}

impl<'a> TypedStr<'a> {
    pub fn colored(&self) -> String {
        let colored_content = match self {
            TypedStr::Null(content) => content
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "null".to_owned())
                .cyan(),
            TypedStr::Bool(content) => content.yellow(),
            TypedStr::Number(content) => content.magenta(),
            TypedStr::String(content) => content.green(),
            TypedStr::Key(content) => content.blue(),
            TypedStr::Escaped(content) => content.red(),
        };
        colored_content.to_string()
    }

    pub fn to_plain(&self) -> &'a str {
        match self {
            TypedStr::Null(content) => content.unwrap_or("null"),
            TypedStr::Bool(content) => content,
            TypedStr::Number(content) => content,
            TypedStr::String(content) => content,
            TypedStr::Key(content) => content,
            TypedStr::Escaped(content) => content,
        }
    }

    pub fn render(&self, color: bool) -> String {
        if color {
            self.colored()
        } else {
            self.to_plain().to_owned()
        }
    }
}
