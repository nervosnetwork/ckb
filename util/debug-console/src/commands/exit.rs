use super::{Error, Return};

pub(crate) async fn execute(_cmd: &str, args: &[String]) -> Result<Return, Error> {
    if args.is_empty() {
        Ok(Return::nop().set_disconnect(true))
    } else {
        Err(Error::TooManyArguments)
    }
}
