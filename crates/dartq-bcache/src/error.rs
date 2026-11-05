//! The crate-wide error type.

/// Every fallible operation in this crate fails with one of these.
///
/// Variants carry string payloads naming the specific path, key, or value
/// involved; callers branch on the variant, humans read the payload.
#[derive(Debug, thiserror::Error)]
pub enum BcacheError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt store: {0}")]
    Corrupt(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, BcacheError>;
