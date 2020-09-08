use std::{convert::TryFrom, fmt, str::FromStr};

use crate::{Arguments, Error, SubCmd, SubCmdName};

#[derive(Debug, Clone)]
pub struct Command {
    name: String,
    subcmd: SubCmd,
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?} {}", self.name, self.subcmd)?;
        Ok(())
    }
}

impl TryFrom<Arguments> for Command {
    type Error = Error;
    fn try_from(args: Arguments) -> Result<Self, Self::Error> {
        let Arguments { mut items, .. } = args;
        if items.len() < 2 {
            Err(Self::Error::InsufficientArguments)
        } else {
            let name = items.remove(0);
            let subcmd_name = SubCmdName::from_str(&items.remove(0))?;
            let subcmd = match subcmd_name {
                SubCmdName::Help => match items.len() {
                    0 | 1 => Ok(SubCmd::Help(items.pop())),
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Get => match items.len() {
                    0 => Err(Self::Error::InsufficientArguments),
                    1 => Ok(SubCmd::Get(items.remove(0))),
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Set => match items.len() {
                    0 | 1 => Err(Self::Error::InsufficientArguments),
                    2 => {
                        let schema = items.remove(0);
                        let unstrict_json = items.remove(0);
                        let strict_json =
                            handwritten_json::normalize(&unstrict_json).map_err(|_| {
                                Error::BadArgument("the last argument is not valid json".to_owned())
                            })?;
                        Ok(SubCmd::Set(schema, strict_json))
                    }
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Delete => match items.len() {
                    0 => Err(Self::Error::InsufficientArguments),
                    1 => Ok(SubCmd::Delete(items.remove(0))),
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Reset => match items.len() {
                    0 => Err(Self::Error::InsufficientArguments),
                    1 => Ok(SubCmd::Reset(items.remove(0))),
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Count => match items.len() {
                    0 => Err(Self::Error::InsufficientArguments),
                    1 => Ok(SubCmd::Count(items.remove(0))),
                    _ => Err(Self::Error::TooManyArguments),
                },
                SubCmdName::Clear => match items.len() {
                    0 => Err(Self::Error::InsufficientArguments),
                    1 => Ok(SubCmd::Clear(items.remove(0))),
                    _ => Err(Self::Error::TooManyArguments),
                },
            }?;
            Ok(Self { name, subcmd })
        }
    }
}

impl Command {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn subcmd(&self) -> &SubCmd {
        &self.subcmd
    }
}
