use thiserror::Error;

// This error is used to collect multiple eyre Reports into one
#[derive(Debug, Error)]
#[error("{0}")]
pub(crate) struct StrError(pub(crate) String);
