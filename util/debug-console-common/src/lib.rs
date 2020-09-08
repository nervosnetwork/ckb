mod arguments;
mod command;
mod error;
mod request;
mod response;
mod subcommand;
pub(crate) mod utilities;

pub use arguments::Arguments;
pub use command::Command;
pub use error::Error;
pub use request::Request;
pub use response::Response;
pub use subcommand::{SubCmd, SubCmdName};

#[macro_export]
macro_rules! failure {
    ($message:ident) => { $crate::Response::new_custom_failure($message.into()) };
    ($message:literal) => { $crate::Response::new_custom_failure($message.to_owned()) };
    ($($token:tt)+) => {{
        let errmsg = format!($($token)+);
        $crate::Response::new_custom_failure(errmsg)
    }}
}

#[macro_export]
macro_rules! success {
    () => { $crate::Response::new_success(String::new()) };
    ($message:ident) => { $crate::Response::new_success($message.into()) };
    ($message:literal) => { $crate::Response::new_success($message.to_owned()) };
    ($($token:tt)+) => {{
        let output = format!($($token)+);
        $crate::Response::new_success(output)
    }}
}
