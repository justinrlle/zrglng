//! Defines errors as handled by the application.
//! TODO: use snafu or something else to have richer information

#[derive(Debug, Clone)]
pub struct ErrMsg(String);

impl ErrMsg {
    pub fn new<T: std::convert::Into<String>>(msg: T) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ErrMsg {}
