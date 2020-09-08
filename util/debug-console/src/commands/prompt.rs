use super::{Error, Return};

pub(crate) async fn execute(_cmd: &str, args: &[String]) -> Result<Return, Error> {
    match args.len() {
        0 => Err(Error::InsufficientArguments),
        1 => match args[0].as_str() {
            "on" | "true" => Ok(Return::nop().set_has_prompt(true)),
            "off" | "false" => Ok(Return::nop().set_has_prompt(false)),
            arg => Err(Error::BadArgument(arg.to_owned())),
        },
        _ => Err(Error::TooManyArguments),
    }
}
