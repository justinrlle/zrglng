//! Defines errors as handled by the application.
//! TODO: use snafu or something else to have richer information

use std::{convert::Into, error::Error, fmt};

#[derive(Debug)]
pub struct ErrMsg {
    msg: String,
    source: Option<Box<dyn Error + Sync + Send>>,
}

impl ErrMsg {
    pub fn new<T: std::convert::Into<String>>(msg: T) -> Self {
        Self {
            msg: msg.into(),
            source: None,
        }
    }

    pub fn with_source<T: Into<String>>(msg: T, source: Box<dyn Error + Sync + Send>) -> Self {
        Self {
            msg: msg.into(),
            source: Some(source),
        }
    }
}

impl fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl Error for ErrMsg {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let Some(e) = self.source.as_ref() {
            Some(e.as_ref())
        } else {
            None
        }
    }
}

#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => ({
        $crate::error::ErrMsg::new(format!($($arg)*))
    })
}

#[macro_export]
macro_rules! err_of {
    ($source:expr, $($arg:tt)*) => ({
        $crate::error::ErrMsg::with_source(format!($($arg)*), $source.into())
    })
}

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => ({
        return Err(err!($($arg)*).into());
    })
}
