use std::convert::TryInto;

use super::{Error, Return};
use crate::{
    common::{Arguments, Request, Response},
    SERVICES,
};

pub(crate) async fn execute(arguments: Arguments) -> Result<Return, Error> {
    let args = arguments.iter();
    let cmd = args[0].as_str();
    let mut sender = SERVICES
        .read()
        .get(cmd)
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::UnsupportedCommand(cmd.to_owned()))?;
    let (request, receiver) = Request::build(arguments.try_into()?);
    sender
        .send(request)
        .await
        .map_err(|error| Error::SendRequest(error.to_string()))?;
    let response = receiver
        .await
        .map_err(|error| Error::RecvResponse(error.to_string()))?;
    let ret = Return::nop();
    match response {
        Response::Success(output) => Ok(ret.set_output(Some(output))),
        Response::Failure(reason) => return Err(reason),
    }
}
