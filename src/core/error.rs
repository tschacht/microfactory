use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The requested operation is invalid in the current state.
    InvalidState(String),
    /// A configuration error occurred.
    Config(String),
    /// A persistence layer error (database, file system, etc.).
    Persistence(String),
    /// An error occurred in the LLM provider.
    LlmProvider {
        provider: String,
        details: String,
        retryable: bool,
    },
    /// An error occurred while rendering a template.
    TemplateRendering(String),
    /// An error occurred during file system operations.
    FileSystem(String),
    /// A red flag was raised during validation.
    RedFlag { flagger: String, reason: String },
    /// A generic system or unknown error.
    System(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidState(msg) => write!(f, "Invalid state: {msg}"),
            Error::Config(msg) => write!(f, "Configuration error: {msg}"),
            Error::Persistence(msg) => write!(f, "Persistence error: {msg}"),
            Error::LlmProvider {
                provider, details, ..
            } => {
                write!(f, "LLM error ({provider}): {details}")
            }
            Error::TemplateRendering(msg) => write!(f, "Template error: {msg}"),
            Error::FileSystem(msg) => write!(f, "File system error: {msg}"),
            Error::RedFlag { flagger, reason } => {
                write!(f, "Red flag raised by {flagger}: {reason}")
            }
            Error::System(msg) => write!(f, "System error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
