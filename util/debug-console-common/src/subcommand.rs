use std::{fmt, str::FromStr};

use crate::Error;

#[derive(Debug, Clone, Copy)]
pub enum SubCmdName {
    Help,
    Get,
    Set,
    Delete,
    Reset,
    Count,
    Clear,
}

#[derive(Debug, Clone)]
pub enum SubCmd {
    Help(Option<String>),
    Get(String),
    Set(String, String),
    Delete(String),
    Reset(String),
    Count(String),
    Clear(String),
}

impl fmt::Display for SubCmdName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Help => write!(f, "help"),
            Self::Get => write!(f, "get"),
            Self::Set => write!(f, "set"),
            Self::Delete => write!(f, "delete"),
            Self::Reset => write!(f, "reset"),
            Self::Count => write!(f, "count"),
            Self::Clear => write!(f, "clear"),
        }
    }
}

impl fmt::Display for SubCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Help(ref arg0_opt) => {
                if let Some(ref arg0) = arg0_opt {
                    write!(f, "help {:?}", arg0)
                } else {
                    write!(f, "help")
                }
            }
            Self::Get(arg0) => write!(f, "get {:?}", arg0),
            Self::Set(arg0, arg1) => write!(f, "set {:?} {:?}", arg0, arg1),
            Self::Delete(arg0) => write!(f, "delete {:?}", arg0),
            Self::Reset(arg0) => write!(f, "reset {:?}", arg0),
            Self::Count(arg0) => write!(f, "count {:?}", arg0),
            Self::Clear(arg0) => write!(f, "clear {:?}", arg0),
        }
    }
}

impl FromStr for SubCmdName {
    type Err = Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "help" => Ok(Self::Help),
            "get" => Ok(Self::Get),
            "set" => Ok(Self::Set),
            "delete" => Ok(Self::Delete),
            "reset" => Ok(Self::Reset),
            "count" => Ok(Self::Count),
            "clear" => Ok(Self::Clear),
            cmd => Err(Self::Err::UnknownSubCommand(cmd.to_owned())),
        }
    }
}

impl SubCmd {
    pub fn name(&self) -> SubCmdName {
        match *self {
            Self::Help(..) => SubCmdName::Help,
            Self::Get(..) => SubCmdName::Get,
            Self::Set(..) => SubCmdName::Set,
            Self::Delete(..) => SubCmdName::Delete,
            Self::Reset(..) => SubCmdName::Reset,
            Self::Count(..) => SubCmdName::Count,
            Self::Clear(..) => SubCmdName::Clear,
        }
    }
}
