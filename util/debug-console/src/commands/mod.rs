use crate::common::{Arguments, Error};

mod exit;
mod prompt;
mod service;

#[derive(Debug, Clone, Default)]
pub(crate) struct Return {
    output: Option<String>,
    disconnect: bool,
    has_prompt: Option<bool>,
}

pub(crate) async fn execute(arguments: Arguments) -> Result<Return, Error> {
    let args = arguments.iter();
    if args.is_empty() {
        Ok(Return::nop())
    } else {
        let cmd = args[0].as_str();
        let args = &args[1..];
        match cmd {
            "prompt" => prompt::execute(cmd, args).await,
            "exit" | "quit" | "bye" => exit::execute(cmd, args).await,
            _ => service::execute(arguments).await,
        }
    }
}

impl Return {
    pub(crate) fn nop() -> Self {
        Self {
            output: None,
            disconnect: false,
            has_prompt: None,
        }
    }

    pub(crate) fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    pub(crate) fn set_output(mut self, output: Option<String>) -> Self {
        self.output = output;
        self
    }

    pub(crate) fn disconnect(&self) -> bool {
        self.disconnect
    }

    pub(crate) fn set_disconnect(mut self, disconnect: bool) -> Self {
        self.disconnect = disconnect;
        self
    }

    pub(crate) fn has_prompt(&self) -> Option<bool> {
        self.has_prompt
    }

    pub(crate) fn set_has_prompt(mut self, has_prompt: bool) -> Self {
        self.has_prompt = Some(has_prompt);
        self
    }
}
