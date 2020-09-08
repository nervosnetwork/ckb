use crate::Error;

#[derive(Debug, Clone)]
pub enum Response {
    Success(String),
    Failure(Error),
}

impl Response {
    pub fn new_success(output: String) -> Self {
        Self::Success(output)
    }

    pub fn new_custom_failure(output: String) -> Self {
        Self::Failure(Error::Custom(output))
    }
}
