use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The sign-in is missing, expired or was revoked: `t3wow login` again.
    AuthRequired(String),
    /// A tool refused the request (`OrchestratorMcpFailure`) or rejected its
    /// arguments. Retrying the same call cannot succeed.
    Tool { tool: String, code: String, message: String },
    /// Network, protocol or file trouble. Usually worth retrying.
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn other(message: impl Into<String>) -> Self {
        Error::Other(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::AuthRequired(message) | Error::Other(message) => f.write_str(message),
            Error::Tool { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Other(error.to_string())
    }
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        Error::Other(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Other(format!("unexpected response: {error}"))
    }
}

impl From<crate::lua::LuaParseError> for Error {
    fn from(error: crate::lua::LuaParseError) -> Self {
        Error::Other(error.0)
    }
}
